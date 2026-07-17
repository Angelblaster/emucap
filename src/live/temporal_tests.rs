use super::finish_with_cleanup;

fn combine(primary: Option<&'static str>, cleanup: &'static str) -> &'static str {
    match primary {
        Some(_) => "primary+cleanup",
        None => cleanup,
    }
}

#[test]
fn successful_effect_is_not_completed_when_cleanup_fails() {
    assert_eq!(
        finish_with_cleanup(Ok::<_, &'static str>(7), Err("cleanup"), combine),
        Err("cleanup")
    );
}

#[test]
fn dual_failure_keeps_both_failure_classes() {
    assert_eq!(
        finish_with_cleanup::<(), _>(Err("primary"), Err("cleanup"), combine),
        Err("primary+cleanup")
    );
}

#[test]
fn primary_failure_survives_successful_cleanup() {
    assert_eq!(
        finish_with_cleanup::<(), _>(Err("primary"), Ok(()), combine),
        Err("primary")
    );
}
