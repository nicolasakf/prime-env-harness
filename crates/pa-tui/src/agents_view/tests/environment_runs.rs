//! The Environments column's feed: how a landed fetch folds into the view.

use super::*;

fn live_run(builder: &str) -> pa_types::environments::InstanceRun {
    serde_json::from_value(serde_json::json!({
        "instance": {
            "id": "i1", "definition_id": "d1", "version": "v1",
            "policy": { "backend": "local" },
        },
        "phase": "running",
        "reason": null,
        "builder_session_id": builder,
    }))
    .expect("run")
}

#[test]
fn a_failed_fetch_keeps_the_last_runs_and_reports_on_the_status_line() {
    let (mut mode, _) = mode_with_row("builder", "mock-1");
    assert!(!mode.apply_environment_runs(Ok(vec![live_run("s1")])));
    assert_eq!(mode.environment_runs, vec![live_run("s1")]);

    assert!(!mode.apply_environment_runs(Err("timed out".to_string())));
    assert_eq!(mode.environment_runs, vec![live_run("s1")]);
    assert_eq!(
        mode.status_text(),
        Some("Environment runs unavailable: timed out")
    );
}
