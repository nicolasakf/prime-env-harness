use pa_environments::{ActiveInstance, EnvironmentManager, StaleView, ViewGuard};
use pa_types::environments::{ExecutionPolicy, SeatId};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn seat(name: &str) -> SeatId {
    SeatId::try_from(name.to_owned()).unwrap()
}

/// A stateless two-seat trading environment, entered with default setup.
async fn market(manager: &EnvironmentManager) -> Arc<ActiveInstance> {
    let draft = manager
        .store()
        .create_draft(
            "versions",
            pa_types::environments::EnvironmentLanguage::Python,
            /*python_execution*/ false,
            &ExecutionPolicy::default(),
        )
        .unwrap();
    for (name, source) in [
        (
            "environment.json",
            include_str!("../fixtures/card_market/environment.json"),
        ),
        (
            "environment.py",
            include_str!("../fixtures/card_market/environment.py"),
        ),
        (
            "checks.py",
            include_str!("../fixtures/card_market/checks.py"),
        ),
    ] {
        std::fs::write(draft.join(name), source).unwrap();
    }
    let report = manager
        .validate(draft.to_str().unwrap(), ExecutionPolicy::default())
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report.diagnostics);
    manager.save(draft.to_str().unwrap(), "market").unwrap();
    manager
        .enter("market", None, ExecutionPolicy::default())
        .await
        .unwrap()
}

async fn act(
    active: &ActiveInstance,
    who: &str,
    action: &str,
    arguments: serde_json::Value,
    id: &str,
    guard: ViewGuard,
) -> anyhow::Result<serde_json::Value> {
    active
        .invoke_guarded(
            &seat(who),
            action,
            arguments,
            id,
            CancellationToken::new(),
            guard,
        )
        .await
}

/// The version a seat's model request is generated from changes only when an engine
/// operation commits, and a refused call leaves the engine and disk untouched.
#[tokio::test]
async fn a_mutation_bound_to_an_old_view_is_refused_before_the_environment_runs() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    let (first, view) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    // Reads create no receipt, so they never invalidate a view.
    assert_eq!(
        active
            .agent_view_versioned(&seat("player-2"))
            .await
            .unwrap()
            .0,
        first
    );
    assert_eq!(
        active.observe_versioned(&seat("player-2")).await.unwrap().0,
        first
    );
    assert_eq!(view["available_actions"], json!(["open_market"]));

    act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":43,"ask":45}),
        "open",
        ViewGuard::Version(first),
    )
    .await
    .unwrap();
    let after = active.version().await;
    assert_eq!(after, first + 1);

    // Player 2 acts on the view it was shown before player 1's quote existed.
    let before = active.checkpoint().await;
    let error = act(
        &active,
        "player-2",
        "improve_market",
        json!({"bid":43,"ask":44}),
        "stale-improve",
        ViewGuard::Version(first),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<StaleView>(),
        Some(&StaleView {
            expected: first,
            current: after
        })
    );
    assert_eq!(active.checkpoint().await, before);
    assert!(!active.path().join("operations/stale-improve.json").exists());

    // The refusal is not remembered: the same call, bound to the fresh view, commits.
    let (fresh, view) = active
        .agent_view_versioned(&seat("player-2"))
        .await
        .unwrap();
    assert_eq!(fresh, after);
    assert_eq!(view["market"]["bid"], 43);
    act(
        &active,
        "player-2",
        "improve_market",
        json!({"bid":43,"ask":44}),
        "stale-improve",
        ViewGuard::Version(fresh),
    )
    .await
    .unwrap();
    active.leave().await.unwrap();
}

/// A buy that was illegal when its model request began can never become legal by
/// the time it arrives: the seat that watched its own quote sit alone is refused.
#[tokio::test]
async fn an_action_that_became_legal_after_the_view_is_still_refused() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    let (open, _) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":43,"ask":45}),
        "open",
        ViewGuard::Version(open),
    )
    .await
    .unwrap();
    // Player 1's last view: its own quote, nothing to do. The peer then improves.
    let (shown, view) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    assert_eq!(view["available_actions"], json!([]));
    let (peer, _) = active
        .agent_view_versioned(&seat("player-2"))
        .await
        .unwrap();
    act(
        &active,
        "player-2",
        "improve_market",
        json!({"bid":43,"ask":44}),
        "peer",
        ViewGuard::Version(peer),
    )
    .await
    .unwrap();
    let before = active.checkpoint().await;
    let error = act(
        &active,
        "player-1",
        "buy",
        json!({}),
        "late-buy",
        ViewGuard::Version(shown),
    )
    .await
    .unwrap_err();
    assert!(error.is::<StaleView>(), "{error:#}");
    assert_eq!(active.checkpoint().await, before);
    // Legal against the current state, and refused only for the view it was bound to.
    let (now, view) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    assert_eq!(view["available_actions"], json!(["buy", "sell"]));
    act(
        &active,
        "player-1",
        "buy",
        json!({}),
        "late-buy",
        ViewGuard::Version(now),
    )
    .await
    .unwrap();
    active.leave().await.unwrap();
}

/// A committed operation ID returns its original receipt before any stale check,
/// including after later operations and a restart; a rejected action also advances
/// the version, since it committed a receipt.
#[tokio::test]
async fn committed_operations_replay_and_rejected_ones_advance_the_version() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    let (first, _) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    let opened = act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":10,"ask":14}),
        "open",
        ViewGuard::Version(first),
    )
    .await
    .unwrap();
    // Not player 2's turn: the environment rejects, and that is still a commit.
    let before = active.version().await;
    act(
        &active,
        "player-1",
        "buy",
        json!({}),
        "illegal",
        ViewGuard::Version(before),
    )
    .await
    .unwrap_err();
    assert_eq!(active.version().await, before + 1);
    let replay = act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":10,"ask":14}),
        "open",
        ViewGuard::Version(first),
    )
    .await
    .unwrap();
    assert_eq!(replay, opened);
    let id = active.instance.id.clone();
    let version = active.version().await;
    active.leave().await.unwrap();
    drop(active);
    let restored = manager.resume(&id).await.unwrap();
    assert_eq!(restored.version().await, version);
    let replay = act(
        &restored,
        "player-1",
        "open_market",
        json!({"bid":10,"ask":14}),
        "open",
        ViewGuard::Version(first),
    )
    .await
    .unwrap();
    assert_eq!(replay, opened);
    let error = act(
        &restored,
        "player-2",
        "buy",
        json!({}),
        "after-restart",
        ViewGuard::Version(first),
    )
    .await
    .unwrap_err();
    assert!(error.is::<StaleView>());
    restored.leave().await.unwrap();
}

/// A rejected action commits a receipt but changes no state, so it cannot starve a
/// seat that is still deciding: a view taken before it stays valid.
#[tokio::test]
async fn a_rejected_action_leaves_other_views_valid() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    let (open, _) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":43,"ask":45}),
        "open",
        ViewGuard::Version(open),
    )
    .await
    .unwrap();
    let (deciding, _) = active
        .agent_view_versioned(&seat("player-2"))
        .await
        .unwrap();
    // Player 1 has nothing to do and keeps trying; every attempt is rejected.
    for attempt in 0..3 {
        let (spent, _) = active
            .agent_view_versioned(&seat("player-1"))
            .await
            .unwrap();
        act(
            &active,
            "player-1",
            "open_market",
            json!({"bid":40,"ask":50}),
            &format!("repeat-{attempt}"),
            ViewGuard::Version(spent),
        )
        .await
        .unwrap_err();
    }
    assert_eq!(active.version().await, deciding + 3);
    act(
        &active,
        "player-2",
        "improve_market",
        json!({"bid":43,"ask":44}),
        "improve",
        ViewGuard::Version(deciding),
    )
    .await
    .unwrap();
    // The accepted action changed the state, so player 1's older view is now stale.
    let error = act(
        &active,
        "player-1",
        "buy",
        json!({}),
        "late",
        ViewGuard::Version(deciding),
    )
    .await
    .unwrap_err();
    assert!(error.is::<StaleView>(), "{error:#}");
    active.leave().await.unwrap();
}

/// Chat is an engine operation too: it invalidates views taken before it.
#[tokio::test]
async fn chat_invalidates_older_views() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    let (first, _) = active
        .agent_view_versioned(&seat("player-1"))
        .await
        .unwrap();
    active
        .input_seat_guarded(&seat("player-1"), "hello", "chat", ViewGuard::Unbound)
        .await
        .unwrap();
    let error = act(
        &active,
        "player-1",
        "open_market",
        json!({"bid":1,"ask":3}),
        "open",
        ViewGuard::Version(first),
    )
    .await
    .unwrap_err();
    assert!(error.is::<StaleView>());
    active.leave().await.unwrap();
}

/// A stray file beside the definitions (Finder's `.DS_Store`) must not break the catalog.
#[tokio::test]
async fn the_catalog_ignores_stray_files_beside_definitions() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let active = market(&manager).await;
    active.leave().await.unwrap();
    std::fs::write(root.path().join("definitions/.DS_Store"), b"\0").unwrap();
    let listed = manager.store().list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "market");
}
