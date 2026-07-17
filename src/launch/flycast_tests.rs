use super::set_ini;
#[cfg(any(target_os = "macos", windows))]
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn set_ini_replaces_and_inserts_under_config() {
    let mut t = "[config]\naica.Volume = 100\nDynarec.Enabled = yes\n".to_string();
    super::set_ini(&mut t, "Dynarec.Enabled", "no"); // replace
    super::set_ini(&mut t, "Debug.GDBEnabled", "no"); // insert
    assert!(t.contains("Dynarec.Enabled = no"));
    assert!(!t.contains("Dynarec.Enabled = yes"));
    assert!(t.contains("Debug.GDBEnabled = no"));
    assert!(t.contains("aica.Volume = 100")); // preserved
}

#[test]
fn set_ini_adds_config_section_when_absent() {
    let mut t = String::new();
    set_ini(&mut t, "aica.Volume", "0");
    assert!(t.contains("[config]"));
    assert!(t.contains("aica.Volume = 0"));
}

#[test]
fn runtime_binary_uses_emucap_owned_plain_exe_dir() {
    let src = tempfile::tempdir().unwrap();
    let iso = tempfile::tempdir().unwrap();
    let source_bin = src.path().join("Flycast.exe");
    let source_cfg = src.path().join("emu.cfg");
    std::fs::write(&source_bin, "fake flycast").unwrap();
    std::fs::write(&source_cfg, "[config]\naica.Volume = 100\n").unwrap();

    let prepared = super::prepare_runtime_binary(&source_bin, iso.path()).unwrap();

    assert_eq!(prepared.portable_dir, iso.path().join("portable"));
    assert_eq!(prepared.binary, iso.path().join("portable/Flycast.exe"));
    assert!(prepared.binary.is_file());
    assert_eq!(
        std::fs::read_to_string(&source_cfg).unwrap(),
        "[config]\naica.Volume = 100\n"
    );
    assert!(prepared.binary.starts_with(iso.path()));
}

#[cfg(unix)]
#[test]
fn runtime_binary_refuses_symlink_inside_portable_dir() {
    let outside = tempfile::tempdir().unwrap();
    let iso = tempfile::tempdir().unwrap();
    let target_bin = outside.path().join("flycast");
    std::fs::write(&target_bin, "user flycast").unwrap();
    make_executable(&target_bin);
    let portable_dir = iso.path().join("portable");
    std::fs::create_dir_all(&portable_dir).unwrap();
    let portable_link = portable_dir.join("flycast");
    std::os::unix::fs::symlink(&target_bin, &portable_link).unwrap();

    let err = super::prepare_runtime_binary(&portable_link, iso.path()).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(std::fs::symlink_metadata(&portable_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_to_string(&target_bin).unwrap(),
        "user flycast"
    );
}

#[cfg(unix)]
#[test]
fn runtime_binary_refuses_symlinked_parent_inside_portable_dir() {
    let outside = tempfile::tempdir().unwrap();
    let iso = tempfile::tempdir().unwrap();
    let outside_portable = outside.path().join("portable-target");
    std::fs::create_dir_all(&outside_portable).unwrap();
    let target_bin = outside_portable.join("flycast");
    std::fs::write(&target_bin, "user flycast").unwrap();
    make_executable(&target_bin);
    let portable_link = iso.path().join("portable");
    std::os::unix::fs::symlink(&outside_portable, &portable_link).unwrap();
    let apparent_binary = portable_link.join("flycast");

    let err = super::prepare_runtime_binary(&apparent_binary, iso.path()).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::read_to_string(&target_bin).unwrap(),
        "user flycast"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn default_install_candidates_include_macos_app() {
    assert!(super::default_install_candidates().contains(&PathBuf::from(
        "/Applications/Flycast.app/Contents/MacOS/Flycast"
    )));
}

#[test]
fn resolve_binary_accepts_explicit_app_bundle_path() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("Flycast.app");
    let binary = app.join("Contents/MacOS/Flycast");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, b"fake flycast").unwrap();
    #[cfg(unix)]
    make_executable(&binary);

    let old = std::env::var_os("FLYCAST_APP");
    std::env::set_var("FLYCAST_APP", &app);
    let resolved = super::resolve_binary();
    match old {
        Some(v) => std::env::set_var("FLYCAST_APP", v),
        None => std::env::remove_var("FLYCAST_APP"),
    }

    assert_eq!(resolved, Some(binary));
}

#[cfg(windows)]
#[test]
fn default_install_candidates_include_windows_user_installs() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old = std::env::var_os("LOCALAPPDATA");
    let base = PathBuf::from(r"C:\Users\alice\AppData\Local");
    std::env::set_var("LOCALAPPDATA", &base);

    let candidates = super::default_install_candidates();

    match old {
        Some(v) => std::env::set_var("LOCALAPPDATA", v),
        None => std::env::remove_var("LOCALAPPDATA"),
    }
    assert!(candidates.contains(&base.join("Programs/Flycast/Flycast.exe")));
}

#[test]
fn runtime_binary_copies_app_bundle_and_uses_inner_binary() {
    let src = tempfile::tempdir().unwrap();
    let iso = tempfile::tempdir().unwrap();
    let app = src.path().join("Flycast.app");
    let source_bin = app.join("Contents/MacOS/Flycast");
    let source_resource = app.join("Contents/Resources/icon.txt");
    std::fs::create_dir_all(source_bin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(source_resource.parent().unwrap()).unwrap();
    std::fs::write(&source_bin, "fake flycast app").unwrap();
    std::fs::write(&source_resource, "resource").unwrap();

    let prepared = super::prepare_runtime_binary(&source_bin, iso.path()).unwrap();

    assert_eq!(
        prepared.binary,
        iso.path()
            .join("portable/Flycast.app/Contents/MacOS/Flycast")
    );
    assert!(iso
        .path()
        .join("portable/Flycast.app/Contents/Resources/icon.txt")
        .is_file());
    assert_eq!(
        std::fs::read_to_string(&source_bin).unwrap(),
        "fake flycast app"
    );
    assert_eq!(
        std::fs::read_to_string(&source_resource).unwrap(),
        "resource"
    );
}
