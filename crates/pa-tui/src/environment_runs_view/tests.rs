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

/// Wrap runs as the session's own.
fn own(runs: Vec<InstanceRun>) -> ScopedRuns {
    ScopedRuns {
        own: runs,
        other: Vec::new(),
    }
}

/// A run belongs to the builder that created it: the dock lists the
/// builder's own runs and its subagents', matched on the saved session id
/// that survives restarts or, for older runs, the current active id. Older
/// runs whose active id no longer matches anything are listed apart, and a
/// run another saved session created is left out.
#[test]
fn runs_are_scoped_to_their_creating_builders() {
    let saved = |mut run: InstanceRun, session: &str| {
        run.builder_saved_session_id = Some(session.to_owned());
        run
    };
    let mine = saved(
        run("mine", "running", Some("before-restart"), &["agent"]),
        "s-builder",
    );
    let child = run("child", "paused", Some("a-subagent"), &["agent"]);
    let elsewhere = saved(
        run("other", "running", Some("a-builder"), &["agent"]),
        "s-other",
    );
    let older = run("older", "stopped", Some("gone"), &["agent"]);
    let unattributed = run("unattributed", "running", None, &["agent"]);
    let owners = RunOwners {
        saved_session_ids: vec!["s-builder", "s-subagent"],
        active_session_ids: vec!["a-builder", "a-subagent"],
    };
    assert_eq!(
        ScopedRuns::of(
            &[
                mine.clone(),
                child.clone(),
                elsewhere,
                older.clone(),
                unattributed.clone()
            ],
            &owners
        ),
        ScopedRuns {
            own: vec![mine, child],
            other: vec![older, unattributed],
        }
    );
}

/// The runs no session can be matched to sit under their own heading
/// below the session's runs, and the selection moves through both groups.
#[test]
fn other_runs_list_under_their_own_heading() {
    let theme = Theme::builtin("prime", ColorMode::TrueColor);
    let kb = KeybindingsManager::new();
    let mine = run("mine", "running", Some("b"), &["agent"]);
    let older = run("older", "stopped", Some("gone"), &["agent"]);
    let mut view = EnvironmentRunsView::new(
        ScopedRuns {
            own: vec![mine],
            other: vec![older.clone()],
        },
        24,
    );
    let text = frame_text(&view.render(&theme, 80, &kb));
    let position = |needle: &str| text.iter().position(|row| row.contains(needle));
    assert!(text[1].contains("1 run"), "{text:?}");
    assert!(
        position("cards — mine") < position("Other runs"),
        "{text:?}"
    );
    assert!(
        position("Other runs") < position("cards — older"),
        "{text:?}"
    );
    view.handle_key("down", &kb);
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::View(older.clone())
    );

    let mut view = EnvironmentRunsView::new(
        ScopedRuns {
            own: Vec::new(),
            other: vec![older.clone()],
        },
        24,
    );
    let text = frame_text(&view.render(&theme, 80, &kb));
    assert!(
        text.iter()
            .any(|row| row.contains("No runs from this session")),
        "{text:?}"
    );
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::View(older)
    );
}

/// Enter opens the selected run and `x` stops it; the arrows move the
/// selection first.
#[test]
fn enter_opens_and_x_stops_the_selected_run() {
    let first = run("one", "running", Some("b"), &["agent", "agent"]);
    let second = run("two", "paused", Some("b"), &["agent"]);
    let mut view = EnvironmentRunsView::new(own(vec![first, second.clone()]), 24);
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

/// Ended runs list after the live ones; Enter shows an ended run's
/// transcript instead of attaching, and `x` has nothing to stop.
#[test]
fn ended_runs_follow_the_live_ones_and_only_open_read_only() {
    let ended = run("ended", "stopped", Some("b"), &["agent"]);
    let live = run("live", "paused", Some("b"), &["agent"]);
    let mut view = EnvironmentRunsView::new(own(vec![ended.clone(), live.clone()]), 24);
    let kb = KeybindingsManager::new();
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::Open(live)
    );
    view.handle_key("down", &kb);
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::View(ended)
    );
    assert_eq!(view.handle_key("x", &kb), EnvironmentRunsAction::None);
    let theme = Theme::builtin("prime", ColorMode::TrueColor);
    let text = frame_text(&view.render(&theme, 80, &kb));
    let hint = text.iter().find(|row| row.contains("close")).expect("hint");
    assert!(
        hint.contains("view transcript") && !hint.contains("x stop"),
        "{hint}"
    );
}

/// A refresh keeps the selection on its run even when the order changes, and
/// falls back to the first row once that run has left the list.
#[test]
fn the_selection_follows_its_run_across_refreshes() {
    let first = run("one", "running", Some("b"), &["agent"]);
    let second = run("two", "running", Some("b"), &["agent"]);
    let kb = KeybindingsManager::new();
    let mut view = EnvironmentRunsView::new(own(vec![first.clone(), second.clone()]), 24);
    view.handle_key("down", &kb);
    view.apply_runs(own(vec![second.clone(), first.clone()]));
    assert_eq!(
        view.handle_key("enter", &kb),
        EnvironmentRunsAction::Open(second)
    );
    view.apply_runs(own(vec![first.clone()]));
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
        own(vec![
            run("one", "running", Some("b"), &["agent", "agent", "agent"]),
            run("two", "stalled", Some("b"), &["human", "agent"]),
        ]),
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
