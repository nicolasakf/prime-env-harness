mod support;
use pa_environments::EnvironmentManager;
use pa_types::environments::{EnvironmentLanguage, ExecutionBackend, ExecutionPolicy, SeatId};
use serde_json::json;
use support::LegacyDraft;
use tokio_util::sync::CancellationToken;

// This target is unconditional in the provisioned CI job. Requesting it
// locally without Docker fails; it never skips or falls back to local.
#[tokio::test]
async fn docker_actions_and_python_use_separate_private_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let credentials = root.path().join("auth.json");
    std::fs::write(&credentials, "fake-provider-credential-for-isolation-check").unwrap();
    let manager = EnvironmentManager::new(root.path().join("environments"), None).unwrap();
    for language in [EnvironmentLanguage::Python, EnvironmentLanguage::TypeScript] {
        for python in [false, true] {
            let policy = ExecutionPolicy {
                backend: ExecutionBackend::Docker,
                python_execution: python,
                ..ExecutionPolicy::default()
            };
            let draft = manager
                .store()
                .create_legacy_draft("Docker matrix", language, python, &policy)
                .unwrap();
            let report = manager
                .validate(draft.to_str().unwrap(), policy.clone())
                .await
                .unwrap();
            assert!(report.passed, "{:?}", report.diagnostics);
            let definition = manager
                .save(
                    draft.to_str().unwrap(),
                    &format!("docker-{language:?}-{python}"),
                )
                .unwrap();
            let active = manager.enter(&definition.id, None, policy).await.unwrap();
            assert_eq!(
                active
                    .invoke(
                        &SeatId::legacy_human(),
                        "increment",
                        json!({"amount":1}),
                        "human",
                        CancellationToken::new()
                    )
                    .await
                    .unwrap(),
                json!(1)
            );
            if python {
                let private = active.path().join("workspace/engine-private.json");
                std::fs::write(&private, "private-engine-state").unwrap();
                let code = format!("import os\nfrom pathlib import Path\nassert not Path('/definition/environment.py').exists()\nassert not Path('/definition/environment.ts').exists()\nassert not Path('/workspace/engine-private.json').exists()\nassert not Path({private:?}).exists()\nassert not Path({credentials:?}).exists()\nassert not Path('/var/run/docker.sock').exists()\nassert 'OPENAI_API_KEY' not in os.environ\nassert 'ANTHROPIC_API_KEY' not in os.environ\nassert not os.access('/root/.prime', os.R_OK)\nassert env.invoke('increment', {{'amount': 2}}) == 3\nremembered = 23");
                active
                    .execute_python(&code, "kernel", CancellationToken::new())
                    .await
                    .unwrap();
            }
            let id = active.instance.id.clone();
            active.leave().await.unwrap();
            drop(active);
            let resumed = manager.resume(&id).await.unwrap();
            assert_eq!(
                resumed.observe(&SeatId::legacy_agent()).await.unwrap(),
                json!({"value":if python {3} else {1}})
            );
            if python {
                resumed
                    .execute_python(
                        "assert remembered == 23",
                        "revived",
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
            }
            resumed.leave().await.unwrap();
        }
    }
}
