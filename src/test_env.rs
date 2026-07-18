use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialize tests that read or mutate the process environment.
pub(crate) fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restore the selected environment variables when the test scope ends.
pub(crate) struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

impl EnvGuard {
    pub(crate) fn new(keys: &[&'static str]) -> Self {
        Self(
            keys.iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect(),
        )
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
