/// Finish a temporal operation only after its terminal cleanup has run.
///
/// A successful effect with failed cleanup is not a successful operation. If both the effect and
/// cleanup fail, `combine` preserves both causes in one fail-loud error.
pub fn finish_with_cleanup<T, E>(
    outcome: Result<T, E>,
    cleanup: Result<(), E>,
    combine: impl FnOnce(Option<E>, E) -> E,
) -> Result<T, E> {
    match (outcome, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup_error)) => Err(combine(None, cleanup_error)),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup_error)) => Err(combine(Some(primary), cleanup_error)),
    }
}

#[cfg(test)]
#[path = "temporal_tests.rs"]
mod tests;
