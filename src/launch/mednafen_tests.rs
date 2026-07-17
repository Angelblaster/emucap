use super::*;
#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn copy_run_binary_replaces_existing_copy() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::write(&src, b"new").unwrap();
    std::fs::write(&dst, b"old").unwrap();

    copy_run_binary(&src, &dst).unwrap();

    assert_eq!(std::fs::read(&dst).unwrap(), b"new");
    assert_eq!(std::fs::read(&src).unwrap(), b"new");
}

#[test]
fn repo_local_candidate_is_platform_native() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("adapters/mednafen/work/mednafen/src");
    std::fs::create_dir_all(&src).unwrap();
    let expected = src.join(default_binary_name());
    std::fs::write(&expected, b"fake mednafen").unwrap();
    #[cfg(unix)]
    make_executable(&expected);

    assert_eq!(repo_local_binary(dir.path()).unwrap(), expected);
}

#[cfg(windows)]
#[test]
fn default_install_candidates_include_windows_user_installs() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old = std::env::var_os("LOCALAPPDATA");
    let base = PathBuf::from(r"C:\Users\alice\AppData\Local");
    std::env::set_var("LOCALAPPDATA", &base);

    let candidates = default_install_candidates();

    match old {
        Some(v) => std::env::set_var("LOCALAPPDATA", v),
        None => std::env::remove_var("LOCALAPPDATA"),
    }
    assert!(candidates.contains(&base.join("Programs/Mednafen/mednafen.exe")));
}

#[test]
fn run_copy_preserves_source_binary_name() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join(if cfg!(windows) {
        "mednafen.exe"
    } else {
        "mednafen"
    });
    let run_dir = dir.path().join("run");

    assert_eq!(
        run_binary_path(&src, &run_dir),
        run_dir.join(default_binary_name())
    );
}
