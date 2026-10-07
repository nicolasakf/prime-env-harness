use super::*;
use crate::theme::{ColorMode, Theme};
use serde_json::{json, Value};

fn run(id: &str, phase: &str, builder: Option<&str>, occupants: &[&str]) -> InstanceRun {
    let seats: Vec<Value> = occupants
        .iter()
        .enumerate()
        .map(|(index, occupant)| {
            json!({"id": format!("seat-{index}"), "role": "player", "occupant": occupant})
        })
        .collect();
    let mut value = json!({
        "instance": {
            "id": id,
            "definition_id": "cards",
            "version": "v1",
            "policy": pa_types::environments::ExecutionPolicy::default(),
            "roster": {"seats": seats},
        },
        "phase": phase,
        "reason": null,
    });
    if let Some(builder) = builder {
        value["builder_session_id"] = json!(builder);
    }
    serde_json::from_value(value).unwrap()
}

fn frame_text(frame: &[Line]) -> Vec<String> {
    frame
        .iter()
        .map(|line| line.iter().map(|span| span.content.as_str()).collect())
        .collect()
}

/// A run belongs to the builder that created it, so the dock lists the
/// builder's own runs and its subagents' and nothing another builder or an
/// unattributed run left behind.
#[test]
fn runs_are_scoped_to_their_creating_builders() {
    let mine = run("mine", "running", Some("builder"), &["agent"]);
    let child = run("child", "paused", Some("subagent"), &["agent"]);
    let other = run("other", "running", Some("elsewhere"), &["agent"]);
    let unattributed = run("old", "running", None, &["agent"]);
    assert_eq!(
        runs_of_builders(
            &[mine.clone(), child.clone(), other, unattributed],
            &["builder", "subagent"]
        ),
        vec![mine, child]
    );
}

/// Enter opens the selected run and `x` stops it; the arrows move the
/// selection first.
#[test]
fn enter_opens_and_x_stops_the_selected_run() {
    let first = run("one", "running", Some("b"), &["agent", "agent"]);
    let second = run("two", "paused", Some("b"), &["agent"]);
    let mut view = EnvironmentRunsView::new(vec![first, second.clone()], 24);
    let kb = KeybindingsManager::new();
    assert_eq!(view.handle_key("down", &kb), EnvironmentRunsAction::None);
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::Open(second.clone())
    );
    assert_eq!(
        view.handle_key("x", &kb),
        EnvironmentRunsAction::Stop(second)
    );
    assert_eq!(view.handle_key("escape", &kb), EnvironmentRunsAction::Close);
}

/// A refresh keeps the selection on its run even when the order changes, and
/// falls back to the first row once that run has left the list.
#[test]
fn the_selection_follows_its_run_across_refreshes() {
    let first = run("one", "running", Some("b"), &["agent"]);
    let second = run("two", "running", Some("b"), &["agent"]);
    let kb = KeybindingsManager::new();
    let mut view = EnvironmentRunsView::new(vec![first.clone(), second.clone()], 24);
    view.handle_key("down", &kb);
    view.apply_runs(vec![second.clone(), first.clone()]);
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::Open(second)
    );
    view.apply_runs(vec![first.clone()]);
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::Open(first)
    );
}

/// The builder can only spectate a run whose seats are all agents; a run
/// with a human seat opens through its saved instance session instead.
#[test]
fn only_all_agent_runs_are_spectated() {
    assert!(is_agents_only(&run(
        "a",
        "running",
        None,
        &["agent", "agent"]
    )));
    assert!(!is_agents_only(&run(
        "b",
        "running",
        None,
        &["human", "agent"]
    )));
}

/// One row per run, never per seat: the seat mix and phase ride the row.
#[test]
fn each_run_renders_one_row_with_its_seats_and_phase() {
    let view = EnvironmentRunsView::new(
        vec![
            run("one", "running", Some("b"), &["agent", "agent", "agent"]),
            run("two", "stalled", Some("b"), &["human", "agent"]),
        ],
        24,
    );
    let theme = Theme::builtin("prime", ColorMode::TrueColor);
    let text = frame_text(&view.render(&theme, 80, &KeybindingsManager::new()));
    let rows: Vec<&String> = text.iter().filter(|row| row.contains("cards")).collect();
    assert_eq!(rows.len(), 2);
    assert!(
        rows[0].contains("cards — one")
            && rows[0].contains("3 agents")
            && rows[0].contains("running")
    );
    assert!(
        rows[1].contains("cards — two")
            && rows[1].contains("1 human, 1 agent")
            && rows[1].contains("stalled")
    );
}
