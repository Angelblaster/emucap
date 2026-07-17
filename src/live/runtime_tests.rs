use super::*;

fn manifest(prepared: &PreparedGeneration) -> CurrentManifest {
    prepared.manifest(ManifestSpec {
        adapter: "mesen2".into(),
        system: "snes".into(),
        content: "/games/test.sfc".into(),
        emulator_pid: std::process::id(),
        bridge_pid: None,
        backend_endpoint: None,
        build: Some("test-build".into()),
    })
}

#[test]
fn prepare_writes_private_auth_without_replacing_current() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let prepared = store.prepare(47800).unwrap();

    assert!(store.read_current(47800).unwrap().is_none());
    assert_eq!(
        store
            .read_auth(47800, prepared.launch_id())
            .unwrap()
            .as_deref(),
        Some(prepared.reclaim_token())
    );
    assert!(!prepared.reclaim_token().contains("47800"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(store.auth_path(47800, prepared.launch_id()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn failed_generation_does_not_destroy_previous_current() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let first = store.prepare(47801).unwrap();
    first.commit(&manifest(&first)).unwrap();

    let second = store.prepare(47801).unwrap();
    second.abort().unwrap();

    let current = store.read_current(47801).unwrap().unwrap();
    assert_eq!(current.launch_id, first.launch_id());
    assert!(store.read_auth(47801, first.launch_id()).unwrap().is_some());
    assert!(!store.generation_dir(47801, second.launch_id()).exists());
}

#[test]
fn commit_atomically_switches_current_and_prunes_old_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let first = store.prepare(47802).unwrap();
    first.commit(&manifest(&first)).unwrap();
    let first_dir = store.generation_dir(47802, first.launch_id());

    let second = store.prepare(47802).unwrap();
    second.commit(&manifest(&second)).unwrap();

    assert_eq!(
        store.read_current(47802).unwrap().unwrap().launch_id,
        second.launch_id()
    );
    assert!(!first_dir.exists());
    assert!(store.generation_dir(47802, second.launch_id()).is_dir());
    let temp_entries = fs::read_dir(store.session_dir(47802))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(temp_entries, 0);
}

#[test]
fn commit_rejects_manifest_from_another_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let first = store.prepare(47803).unwrap();
    let second = store.prepare(47803).unwrap();

    let err = second.commit(&manifest(&first)).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(store.read_current(47803).unwrap().is_none());
}

#[test]
fn oversized_capsule_file_is_rejected_before_parsing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let path = store.current_path(47804);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, vec![b'x'; MAX_CAPSULE_FILE_BYTES as usize + 1]).unwrap();

    let err = store.read_current(47804).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn current_manifest_never_serializes_reclaim_token() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let prepared = store.prepare(47805).unwrap();
    prepared.commit(&manifest(&prepared)).unwrap();

    let current = fs::read_to_string(store.current_path(47805)).unwrap();
    assert!(!current.contains(prepared.reclaim_token()));
    assert!(current.contains(prepared.launch_id()));
}

#[test]
fn process_state_requires_matching_start_identity() {
    let captured = capture_process(std::process::id());
    if captured.start_identity.is_none() {
        assert_eq!(process_state(&captured), ProcessState::Unknown);
        return;
    }
    assert_eq!(process_state(&captured), ProcessState::Alive);

    let reused = ProcessIdentity {
        pid: captured.pid,
        start_identity: Some("different-start".into()),
    };
    assert_eq!(process_state(&reused), ProcessState::Exited);
}

#[test]
fn current_rejects_path_traversal_launch_id() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let path = store.current_path(47806);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "launch_id": "launch-../../escape",
            "port": 47806,
            "adapter": "mesen2",
            "system": "snes",
            "content": "/game.sfc",
            "emulator": {"pid": std::process::id()},
            "created_at_unix_ms": 1
        }))
        .unwrap(),
    )
    .unwrap();

    let error = store.read_current(47806).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[cfg(unix)]
#[test]
fn auth_reader_refuses_symlink() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().unwrap();
    let store = RuntimeStore::new(tmp.path().join("sessions"));
    let prepared = store.prepare(47807).unwrap();
    let auth = store.auth_path(47807, prepared.launch_id());
    let outside = tmp.path().join("outside-secret");
    fs::write(&outside, "secret").unwrap();
    fs::remove_file(&auth).unwrap();
    symlink(&outside, &auth).unwrap();

    let error = store.read_auth(47807, prepared.launch_id()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}
