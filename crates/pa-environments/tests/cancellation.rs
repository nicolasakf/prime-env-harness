mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionBackend, ExecutionPolicy, SeatId};
use serde_json::json;
use std::fs;
use std::time::Duration;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

#[tokio::test]
// The readiness files prove dispatch happened before cancellation; never replay it.
#[allow(clippy::too_many_lines)]
async fn cancellation_stops_workers_and_retains_uncertain_intents() {
    let root = tempfile::tempdir().unwrap();
    let manager = EnvironmentManager::new(root.path().into(), None).unwrap();
    let backends = if cfg!(feature = "docker-integration") {
        vec![ExecutionBackend::Local, ExecutionBackend::Docker]
    } else {
        vec![ExecutionBackend::Local]
    };
    for backend in backends {
        let policy = ExecutionPolicy {
            backend,
            python_execution: true,
            ..ExecutionPolicy::default()
        };
        let draft = manager
            .store()
            .create_legacy_draft("cancellation", EnvironmentLanguage::Python, true, &policy)
            .unwrap();
        let code = fs::read_to_string(draft.join("environment.py")).unwrap();
        let code = "import subprocess\nimport sys\nimport time\nfrom pathlib import Path\n".to_owned() + &code.replace("self.value += arguments[\"amount\"]", "if arguments['amount'] == 99:\n            child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(600)'])\n            Path('engine-ready').write_text(str(child.pid))\n            while True:\n                time.sleep(1)\n        self.value += arguments[\"amount\"]");
        fs::write(draft.join("environment.py"), code).unwrap();
        let report = manager
            .validate(draft.to_str().unwrap(), policy.clone())
            .await
            .unwrap();
        assert!(report.passed, "{report:?}");
        let definition = manager
            .save(draft.to_str().unwrap(), &format!("cancel-{backend:?}"))
            .unwrap();
        for (python, by_leave, checkpoint) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (true, true, true),
        ] {
            let active = manager
                .enter(&definition.id, None, policy.clone())
                .await
                .unwrap();
            let id = active.instance.id.clone();
            let signal = CancellationToken::new();
            let pending = {
                let active = active.clone();
                let signal = signal.clone();
                tokio::spawn(async move {
                    if python {
                        let code = if checkpoint {
                            "from pathlib import Path\nimport time\nclass SlowSnapshot:\n    def __reduce__(self):\n        Path('kernel-ready').write_text('ready')\n        while True:\n            time.sleep(1)\nslow_snapshot = SlowSnapshot()"
                        } else {
                            "from pathlib import Path\nimport time\nPath('kernel-ready').write_text('ready')\nwhile True:\n    time.sleep(1)"
                        };
                        active.execute_python(code, "cancel-kernel", signal).await
                    } else {
                        active
                            .invoke(
                                &SeatId::legacy_agent(),
                                "increment",
                                json!({"amount":99}),
                                "cancel-engine",
                                signal,
                            )
                            .await
                    }
                })
            };
            let marker = active.path().join(if python {
                "participant/kernel-ready"
            } else {
                "workspace/engine-ready"
            });
            tokio::time::timeout(Duration::from_secs(15), async {
                while !marker.exists() {
                    assert!(!pending.is_finished(), "worker stopped before readiness");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("worker readiness");
            if by_leave {
                assert!(tokio::time::timeout(Duration::from_secs(5), active.leave())
                    .await
                    .expect("leaving must cancel active work")
                    .is_err());
            } else {
                signal.cancel();
            }
            let result = tokio::time::timeout(Duration::from_secs(5), pending)
                .await
                .expect("cancellation must end active work")
                .unwrap();
            assert!(result.is_err());
            if !by_leave {
                assert!(
                    active.leave().await.is_err(),
                    "cancelled mutations require recovery"
                );
            }
            drop(active);
            if !python && backend == ExecutionBackend::Local {
                let child: u32 = fs::read_to_string(marker).unwrap().parse().unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while pa_types::platform::process::is_process_alive(child).unwrap() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("local worker grandchildren must stop too");
            }
            assert_eq!(
                manager.inspect(&id, &SeatId::legacy_human()).await.unwrap(),
                json!({"value":0})
            );
            assert!(
                manager.resume(&id).await.is_err(),
                "unresolved operations cannot be replayed"
            );
        }
    }
}
