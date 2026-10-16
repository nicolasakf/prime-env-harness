#![cfg(unix)]
#[path = "support/environment_daemon.rs"]
mod support;
use serde_json::{json, Value};
use support::start;

#[test]
fn human_action_wakes_two_seats_without_a_prompt_and_keeps_their_text_private() {
    relay_run(Scenario::Human);
}
#[test]
fn agents_only_run_finishes_without_a_human_and_exposes_only_public_events() {
    relay_run(Scenario::Agents);
}

#[derive(Clone, Copy)]
enum Scenario {
    Human,
    Agents,
    SeatCrash,
    HostCrash,
    SupervisorCrash,
    Builder,
    BuilderStop,
    StopAfterLeave,
    Python,
    PythonPause,
    PythonStop,
    SealedBid,
}
#[test]
fn seat_crash_replays_the_wake_without_duplicate_actions() {
    relay_run(Scenario::SeatCrash);
}
#[test]
fn host_crash_readopts_seats_and_keeps_delivery_order() {
    relay_run(Scenario::HostCrash);
}
#[test]
fn supervisor_restart_revives_a_model_less_host() {
    relay_run(Scenario::SupervisorCrash);
}

#[test]
fn builder_kernel_provisions_and_reads_a_run_without_player_tools() {
    relay_run(Scenario::Builder);
}

#[test]
fn builder_kernel_pauses_then_stops_a_live_run() {
    relay_run(Scenario::BuilderStop);
}

#[test]
fn builder_prompt_lists_and_stops_a_run_whose_host_has_closed() {
    relay_run(Scenario::StopAfterLeave);
}

#[test]
fn python_actions_use_the_same_host_admission_as_seat_tools() {
    relay_run(Scenario::Python);
}

#[test]
fn simultaneous_sealed_bids_finish_without_exposing_private_observations() {
    relay_run(Scenario::SealedBid);
}

#[test]
fn pausing_a_running_python_cell_does_not_wait_for_it_to_finish() {
    relay_run(Scenario::PythonPause);
}

#[test]
fn stopping_a_running_python_cell_settles_before_disposing_its_seat() {
    relay_run(Scenario::PythonStop);
}

// All crash/control scenarios use the same provisioning and observable assertions.
#[allow(clippy::too_many_lines)]
fn relay_run(scenario: Scenario) {
    let sealed = matches!(scenario, Scenario::SealedBid);
    let role = if sealed { "bidder" } else { "player" };
    let human = !matches!(
        scenario,
        Scenario::Agents | Scenario::Builder | Scenario::BuilderStop | Scenario::SealedBid
    );
    let root = tempfile::tempdir().unwrap();
    // Surviving faux providers need a second action turn when a supervisor
    // outage loses the original tool reply; operation IDs dedupe either case.
    let python = matches!(
        scenario,
        Scenario::Python | Scenario::PythonPause | Scenario::PythonStop
    );
    let action = if matches!(scenario, Scenario::PythonPause | Scenario::PythonStop) {
        json!({"content":[{"type":"toolCall","id":"occupied","name":"ipython","arguments":{"code":"from pathlib import Path
import time
Path('ready').write_text('ready')
while True:
    time.sleep(1)"}}]})
    } else if matches!(scenario, Scenario::Python) {
        json!({"content":[{"type":"toolCall","id":"pass-baton","name":"ipython","arguments":{"code":"assert isinstance(env.invoke('pass', {}), int)\ntry:\n    env.invoke('pass', {})\nexcept RuntimeError as error:\n    assert 'stale view' in str(error), error\n    print('SECOND-INVOKE-REFUSED')\nelse:\n    raise AssertionError('a cell acts on one view')"}}]})
    } else if sealed {
        json!({"content":[{"type":"thinking","thinking":"PRIVATE-THINKING"},{"type":"toolCall","id":"pass-baton","name":"env_bid","arguments":{"amount":7}}]})
    } else {
        json!({"delayMs":800,"content":[{"type":"thinking","thinking":"PRIVATE-THINKING"},{"type":"toolCall","id":"pass-baton","name":"env_pass","arguments":{}}]})
    };
    // Simultaneous bidders invalidate each other's views: a seat whose bid is refused
    // as stale reconsiders from the state it is handed and bids again. A seat that
    // was not refused makes the second call too, and the environment rejects it.
    let responses = if sealed {
        json!([
            action,
            action,
            "PRIVATE-ASSISTANT-SECRET",
            action,
            action,
            "PRIVATE-ASSISTANT-SECRET",
            action,
            action,
            "PRIVATE-ASSISTANT-SECRET"
        ])
    } else {
        json!([
            action,
            "PRIVATE-ASSISTANT-SECRET",
            action,
            "PRIVATE-ASSISTANT-SECRET",
            action,
            "PRIVATE-ASSISTANT-SECRET"
        ])
    };
    let mut script = json!({"engine":"faux","responses":responses});
    if matches!(scenario, Scenario::Builder | Scenario::BuilderStop) {
        let code = if matches!(scenario, Scenario::BuilderStop) {
            "import environments\nrun = await environments.start_instance('relay', [{'id': 'player-' + str(i), 'role': 'player', 'occupant': 'agent'} for i in range(1, 4)], limits={'max_operations': 20})\npaused = await environments.pause_instance(run['instance_id'])\nassert paused['run']['phase'] == 'paused', paused\nassert (await environments.instance_status(run['instance_id']))['phase'] == 'paused'\nresumed = await environments.resume_instance(run['instance_id'])\nassert resumed['run']['phase'] == 'running', resumed\npaused = await environments.pause_instance(run['instance_id'])\nassert paused['run']['phase'] == 'paused', paused\nstopped = await environments.stop_instance(run['instance_id'])\nassert stopped['run']['phase'] == 'stopped', stopped\nprint('RUN-STOPPED')"
        } else {
            "import environments\nrun = await environments.start_instance('relay', [{'id': 'player-' + str(i), 'role': 'player', 'occupant': 'agent'} for i in range(1, 4)], limits={'max_operations': 20})\nassert (await environments.instance_status(run['instance_id']))['phase'] in ['running', 'finished']\nawait environments.observe_seat(run['instance_id'], 'player-2')\nawait environments.table_log(run['instance_id'])\nawait environments.seat_transcript(run['instance_id'], 'player-2')\nprint('RUN-PROVISIONED')"
        };
        script["responses"] = json!([{"content":[{"type":"toolCall","id":"provision","name":"ipython","arguments":{"code":code}}]},"BUILDER-DONE","STATUS-RECEIVED"]);
        for index in 1..=3 {
            script["environmentSeats"][format!("player-{index}")] =
                json!({"engine":"faux","responses":[action,"PRIVATE-ASSISTANT-SECRET"]});
        }
    }
    let (mut supervisor, mut client) = start(root.path(), &script);
    let created = client.request(&json!({"type":"create","config":{"cwd":root.path(),"script":root.path().join("faux.json"),"sessionDir":root.path().join("agent/sessions")},"telemetryDisabled":true}));
    assert_eq!(created["success"], true, "{created}");
    let builder = created["data"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let draft = client.environment(
        &builder,
        "create",
        &json!({"description":"relay", "language":if sealed { "typescript" } else { "python" }}),
    )["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = std::path::Path::new(&draft);
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("environment.json")).unwrap()).unwrap();
    manifest["protocol_version"] = json!(2);
    manifest["python_execution"] = json!(python);
    manifest["roles"] = json!([{"name":role,"min_seats":3,"max_seats":3}]);
    std::fs::write(path.join("environment.json"), manifest.to_string()).unwrap();
    if sealed {
        std::fs::write(
            path.join("environment.ts"),
            include_str!("../../pa-environments/fixtures/sealed_bid/environment.ts"),
        )
        .unwrap();
        std::fs::write(path.join("checks.ts"), "").unwrap();
    } else {
        std::fs::write(
            path.join("environment.py"),
            include_str!("../../pa-environments/fixtures/relay/environment.py"),
        )
        .unwrap();
        std::fs::write(path.join("checks.py"), "").unwrap();
    }
    client.environment(
        &builder,
        "policy",
        &json!({"draft":draft,"policy":{"backend":"local", "python_execution": python}}),
    );
    assert_eq!(
        client.environment(&builder, "validate", &json!({"draft":draft}))["passed"],
        true
    );
    client.environment(&builder, "save", &json!({"draft":draft,"id":"relay"}));
    let seats = (1..=3).map(|index| json!({"id":format!("{role}-{index}"),"role":role,"occupant":if human && index == 1 { "human" } else { "agent" }})).collect::<Vec<_>>();
    let entered = if matches!(scenario, Scenario::Builder | Scenario::BuilderStop) {
        assert_eq!(
            client.request(&json!({"type":"attach","activeSessionId":builder}))["success"],
            true
        );
        let result = client.request(&json!({"type":"prompt_and_wait","activeSessionId":builder,"message":"Start the authorized relay run and inspect it."}));
        assert_eq!(result["success"], true, "{result}");
        assert!(
            client
                .events
                .iter()
                .any(|event| event["type"] == "tool_execution_end"
                    && event["toolCallId"] == "provision"
                    && event["isError"] != true
                    && event["result"].to_string().contains(
                        if matches!(scenario, Scenario::BuilderStop) {
                            "RUN-STOPPED"
                        } else {
                            "RUN-PROVISIONED"
                        }
                    )),
            "builder provisioning output: {:?}",
            client.events
        );
        let instances = client.environment(&builder, "instances", &json!({}));
        let id = instances["instances"][0]["id"].as_str().unwrap();
        let host: Value = serde_json::from_slice(
            &std::fs::read(
                root.path()
                    .join("agent/environments/instances")
                    .join(id)
                    .join("host-session.json"),
            )
            .unwrap(),
        )
        .unwrap();
        json!({"session":host,"instance_id":id})
    } else {
        client.environment(
        &builder,
        "enter",
        &json!({"id":"relay","policy":{"backend":"local", "python_execution": python},"roster":{"seats":seats}}),
    )
    };
    let host = entered["session"]["activeSessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        client.request(&json!({"type":"attach","activeSessionId":host}))["success"],
        true
    );
    assert!(
        client.request(&json!({"type":"get_state","activeSessionId":host}))["data"]["model"]
            .is_null(),
        "table hosts carry no model"
    );
    assert_eq!(
        client.request(&json!({"type":"get_state","activeSessionId":host}))["data"]
            ["environmentRole"],
        "host",
        "the agents view hides table hosts"
    );
    let instance = entered["instance_id"].as_str().unwrap();
    let instance_path = root
        .path()
        .join("agent/environments/instances")
        .join(instance);
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(instance_path.join("owner.lock"))
        .unwrap();
    assert!(
        lease.try_lock().is_err(),
        "host holds exclusive instance ownership"
    );
    if matches!(scenario, Scenario::BuilderStop) {
        let status = client.environment(&host, "status", &json!({}));
        assert_eq!(status["run"]["phase"], "stopped");
        assert_eq!(status["run"]["reason"], "builder requested stop");
        assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
        supervisor.0.wait().unwrap();
        return;
    }
    if matches!(scenario, Scenario::StopAfterLeave) {
        client.wait_event(|event| event["type"] == "environment_runs_changed");
        client.environment(&host, "leave", &json!({}));
        let controls = client.environment(&builder, "controls", &json!({}));
        assert_eq!(controls["runs"][0]["instance"]["id"], json!(instance));
        assert_eq!(controls["runs"][0]["phase"], "paused");
        assert_eq!(controls["runs"][0]["builder_session_id"], json!(builder));
        // The run also records the builder's saved session id, which stays
        // the same when the builder reopens under a new active id.
        let saved_session = client.request(&json!({"type":"get_state","activeSessionId":builder}))
            ["data"]["sessionId"]
            .clone();
        assert!(saved_session.is_string(), "{saved_session}");
        assert_eq!(
            controls["runs"][0]["builder_saved_session_id"],
            saved_session
        );
        let announced = client
            .events
            .iter()
            .filter(|event| event["type"] == "environment_runs_changed")
            .count();
        assert_eq!(
            announced, 2,
            "the first save and pausing the run each announce once"
        );
        let stopped = client.environment(&builder, "stop", &json!({"id":instance}));
        assert_eq!(stopped["run"]["phase"], "stopped");
        assert_eq!(stopped["run"]["reason"], "human requested stop");
        assert_eq!(
            client.environment(&builder, "controls", &json!({}))["runs"],
            json!([])
        );
        let ended = client.environment(&builder, "controls", &json!({"include_ended":true}));
        assert_eq!(ended["runs"][0]["instance"]["id"], json!(instance));
        assert_eq!(ended["runs"][0]["phase"], "stopped");
        let transcript = client.environment(&builder, "transcript", &json!({"id":instance}));
        assert!(
            transcript["messages"].is_array(),
            "the ended run's transcript stays readable: {transcript}"
        );
        assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
        supervisor.0.wait().unwrap();
        return;
    }
    if human {
        client.environment(
            &host,
            "act",
            &json!({"action":"pass","arguments":{},"operation_id":"human-pass"}),
        );
    }
    if matches!(
        scenario,
        Scenario::SeatCrash | Scenario::HostCrash | Scenario::SupervisorCrash
    ) {
        client.wait_event(|event| {
            event["type"] == "environment_seat_state"
                && event["run"]["busy"]["player-2"].as_u64().is_some()
        });
        let seat: Value = serde_json::from_slice(
            &std::fs::read(instance_path.join("seats/player-2/session.json")).unwrap(),
        )
        .unwrap();
        let target = if matches!(scenario, Scenario::SeatCrash) {
            seat["activeSessionId"].as_str().unwrap()
        } else {
            &host
        };
        let descriptor = std::fs::read_dir(root.path().join("agent/daemon-workers"))
            .unwrap()
            .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| std::fs::read(path).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .find(|descriptor| descriptor["rootActiveSessionId"] == target)
            .unwrap();
        if matches!(scenario, Scenario::SupervisorCrash) {
            supervisor.0.kill().unwrap();
            supervisor.0.wait().unwrap();
        }
        assert!(std::process::Command::new("kill")
            .arg("-KILL")
            .arg(descriptor["pid"].as_u64().unwrap().to_string())
            .status()
            .unwrap()
            .success());
        if matches!(scenario, Scenario::SupervisorCrash) {
            supervisor.1 = std::path::PathBuf::new();
            (supervisor, client) = start(root.path(), &script);
            let roster = client.request(&json!({"type":"roster_subscribe"}));
            if !roster["data"]["roster"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["summary"]["activeSessionId"] == host)
            {
                client.wait_event(|event| {
                    event["type"] == "roster_update"
                        && event["changed"].as_array().is_some_and(|entries| {
                            entries
                                .iter()
                                .any(|entry| entry["summary"]["activeSessionId"] == host)
                        })
                });
            }
            let attached = client.request(&json!({"type":"attach","activeSessionId":host}));
            assert_eq!(attached["success"], true, "{attached}");
        }
    }
    if matches!(scenario, Scenario::HostCrash) {
        client.wait_event(|event| {
            event["type"] == "environment_status" && event["run"]["phase"] == "finished"
        });
    }
    if matches!(scenario, Scenario::PythonPause | Scenario::PythonStop) {
        let ready = instance_path.join("seats/player-2/participant/ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !ready.exists() {
            assert!(std::time::Instant::now() < deadline, "Python readiness");
            std::thread::yield_now();
        }
        let stop = matches!(scenario, Scenario::PythonStop);
        let paused = client.environment(&host, if stop { "stop" } else { "pause" }, &json!({}));
        assert_eq!(
            paused["run"]["phase"],
            if stop { "stopped" } else { "paused" }
        );
        if stop {
            assert_eq!(client.request(&json!({"type":"env","activeSessionId":host,"operation":"resume_run","parameters":{}}))["success"], false);
        }
    }
    let current = client.environment(&host, "status", &json!({}));
    if current["run"]["phase"] != "finished"
        && !matches!(scenario, Scenario::PythonPause | Scenario::PythonStop)
    {
        client.wait_event(|event| {
            event["type"] == "environment_status" && event["run"]["phase"] == "finished"
        });
    }
    let checkpoint: Value =
        serde_json::from_slice(&std::fs::read(instance_path.join("checkpoint.json")).unwrap())
            .unwrap();
    if sealed {
        assert_eq!(checkpoint["snapshot"]["bids"].as_object().unwrap().len(), 3);
    } else {
        assert_eq!(
            checkpoint["snapshot"]["turn"],
            if matches!(scenario, Scenario::PythonPause | Scenario::PythonStop) {
                1
            } else {
                3
            }
        );
    }
    if matches!(scenario, Scenario::Python) {
        // Each cell's first invoke committed; its second was refused as stale, because
        // the cell is bound to the view that wrote it and never rebinds.
        let refused = (1..=3)
            .filter(|index| {
                std::fs::read_to_string(
                    instance_path.join(format!("seats/player-{index}/conversation.jsonl")),
                )
                .is_ok_and(|transcript| transcript.contains("SECOND-INVOKE-REFUSED"))
            })
            .count();
        assert_eq!(refused, 3 - usize::from(human));
    }
    let history = client.request(&json!({"type":"get_messages","activeSessionId":host}));
    assert_eq!(history["success"], true);
    if matches!(scenario, Scenario::Agents) {
        // The saved table rows parse as written, and the builder reads back
        // from disk the conversation the live table served (the table may
        // have appended a row since).
        let saved = std::fs::read_to_string(instance_path.join("host-conversation.jsonl")).unwrap();
        for row in saved.lines().skip(1) {
            assert!(
                serde_json::from_str::<pa_daemon::session_store::SessionEntry>(row).is_ok(),
                "unreadable transcript row: {row}"
            );
        }
        let live = history["data"]["messages"].as_array().unwrap().clone();
        let transcript = client.environment(&builder, "transcript", &json!({"id":instance}));
        let read = transcript["messages"].as_array().unwrap();
        assert_eq!(read[..live.len().min(read.len())], live[..]);
    }
    let public = format!("{history} {:?}", client.events);
    assert!(!public.contains("PRIVATE-THINKING"));
    assert!(!public.contains("PRIVATE-ASSISTANT-SECRET"));
    if sealed {
        assert!(!public.contains("your_bid"));
    }
    let seat_session: Value = serde_json::from_slice(
        &std::fs::read(instance_path.join(format!("seats/{role}-2/session.json"))).unwrap(),
    )
    .unwrap();
    let seat_id = seat_session["activeSessionId"].as_str().unwrap();
    // Direct seat clients cannot attach, prompt, or inspect private history.
    for command in ["attach", "prompt", "get_messages"] {
        assert_eq!(
            client.request(&json!({"type":command,"activeSessionId":seat_id,"message":"bypass"}))
                ["success"],
            false
        );
    }
    assert_eq!(client.request(&json!({"type":"env","activeSessionId":host,"operation":"snapshot","parameters":{"builder_token":"wrong"}}))["success"],false);
    client.environment(&host, "leave", &json!({}));
    assert_eq!(client.request(&json!({"type":"shutdown"}))["success"], true);
    supervisor.0.wait().unwrap();
}
