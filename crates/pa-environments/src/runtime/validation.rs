//! Environment validation responsibility.
use super::{
    bail, content_hash, describe, fs, json, read_json, tempfile_path, validate_policy, value,
    write_json, ActionDescriptor, AgentContext, Arc, CancellationToken, Context,
    EnvironmentLanguage, EnvironmentManager, EnvironmentManifest, ExecutionPolicy, HostCallHandler,
    MissedTurn, MissedTurnCause, Path, Result, SeatId, Uuid, ValidationReport, Value,
    ViewPerspective, Worker, SUPPORTED_PROTOCOL_VERSIONS,
};

impl EnvironmentManager {
    /// Validate a draft in the user-selected execution backend.
    /// # Errors
    /// Returns invalid draft/policy errors; execution failures land in the report.
    pub async fn validate(
        &self,
        selector: &str,
        policy: ExecutionPolicy,
    ) -> Result<ValidationReport> {
        validate_policy(&policy)?;
        self.validate_mounts(&policy)?;
        let draft = self.store.draft(selector)?;
        let history = self
            .store
            .root()
            .join("validation")
            .join(draft.file_name().context("draft id")?);
        if history.exists()
            && fs::read_dir(&history)?
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_name() != "latest.json")
                .count()
                >= 4
        {
            bail!("initial validation plus three repair passes exhausted; create a new draft");
        }
        let hash = content_hash(&draft)?;
        let manifest: EnvironmentManifest = read_json(&draft.join("environment.json"))?;
        let mut report = ValidationReport {
            content_hash: hash.clone(),
            policy: policy.clone(),
            passed: false,
            diagnostics: Vec::new(),
            actions: Vec::new(),
        };
        let result = self.validate_contract(&draft, &manifest, &policy).await;
        match result {
            Ok(actions) => {
                report.actions = actions;
                report.passed = true;
            }
            Err(error) => report.diagnostics.push(format!("{error:#}")),
        }
        if content_hash(&draft)? != hash {
            report.passed = false;
            report
                .diagnostics
                .push("draft changed during validation".into());
        }
        // Each attempt remains available even after the next repair.
        write_json(&history.join(format!("{}.json", Uuid::new_v4())), &report)?;
        write_json(&history.join("latest.json"), &report)?;
        Ok(report)
    }

    pub(super) async fn validate_contract(
        &self,
        source: &Path,
        manifest: &EnvironmentManifest,
        policy: &ExecutionPolicy,
    ) -> Result<Vec<ActionDescriptor>> {
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&manifest.protocol_version)
            || manifest.name.trim().is_empty()
            || manifest.purpose.trim().is_empty()
        {
            bail!("manifest must declare a supported protocol, a name, and a purpose");
        }
        if manifest.protocol_version == 1
            && (!manifest.roles.is_empty() || manifest.direct_messages.is_some())
        {
            bail!("roles and direct_messages require protocol v2");
        }
        if manifest.agent_context == AgentContext::Stateless && manifest.protocol_version != 2 {
            bail!("agent_context stateless requires protocol v2");
        }
        if manifest.missed_turns && manifest.protocol_version != 2 {
            bail!("missed_turns requires protocol v2");
        }
        let roster = crate::default_roster(manifest)?;
        if policy.python_execution && !manifest.python_execution {
            bail!("definition does not declare participant Python capability");
        }
        let prepared = self.prepare(source, manifest.language, policy).await?;
        let scratch = tempfile_path(self.store.root(), "validation-workspace")?;
        let mut worker =
            Worker::start(&prepared, &scratch, manifest.language, policy, false).await?;
        let result = async {
            let arguments = setup_arguments(manifest.setup_schema.as_ref(), json!({}))?;
            let arguments = if manifest.protocol_version == 2 { json!({"seats":roster.seats.iter().map(|seat| json!({"id":seat.id,"role":seat.role})).collect::<Vec<_>>(),"config":arguments}) } else { arguments };
            value(worker.request("initialize", &SeatId::legacy_human(), arguments, CancellationToken::new(), None).await?)?;
            let actions = describe(&mut worker, manifest).await?;
            let before = value(worker.request("snapshot", &SeatId::legacy_human(), json!({}), CancellationToken::new(), None).await?)?;
            for participant in roster.seats.iter().map(|seat| &seat.id) {
                value(worker.request("observe", participant, json!({}), CancellationToken::new(), None).await?)?;
                if manifest.agent_context == AgentContext::Stateless {
                    let view = value(worker.request("agent_view", participant, json!({}), CancellationToken::new(), None).await?)?;
                    super::instance::check_agent_view(&view)?;
                }
            }
            if manifest.protocol_version == 2 {
                let perspectives = std::iter::once(ViewPerspective::Public).chain(
                    roster.seats.iter().map(|seat| ViewPerspective::Seat { seat: seat.id.clone() }),
                );
                for perspective in perspectives {
                    let response = worker.request("frame", &SeatId::legacy_human(), json!({"perspective":perspective}), CancellationToken::new(), None).await?;
                    // A draft scaffolded before live views existed has no runner support for frame.
                    if response["error"] == "unknown environment method" { break; }
                    let frame = value(response)?;
                    if frame.is_null() { break; }
                    super::instance::check_frame(&frame).with_context(|| format!("frame({})", json!(perspective)))?;
                }
            }
            if manifest.missed_turns {
                let seat = roster.seats.first().map(|seat| seat.id.clone()).context("roster seat")?;
                let report = MissedTurn { cause: MissedTurnCause::Idle, misses: 1, limit: 1 };
                // A domain refusal is the environment's call; a missing handler is a defect.
                let response = worker.request("missed_turn", &seat, json!(report), CancellationToken::new(), None).await?;
                if response["error"].as_str().is_some_and(|error| error.ends_with("unknown environment method")) { bail!("manifest declares missed_turns but the environment defines no missed_turn"); }
            }
            // Restoration is tested in a NEW worker, not against its own live state.
            worker.stop().await?;
            worker = Worker::start(&prepared, &scratch, manifest.language, policy, false).await?;
            value(worker.request("restore", &SeatId::legacy_human(), before.clone(), CancellationToken::new(), None).await?)?;
            let after = value(worker.request("snapshot", &SeatId::legacy_human(), json!({}), CancellationToken::new(), None).await?)?;
            if before != after { bail!("snapshot does not survive restoration in a fresh worker"); }
            if manifest.python_execution && policy.python_execution {
                let kernel_source = self.kernel_source(policy).await?;
                let kernel_scratch = tempfile_path(self.store.root(), "validation-kernel")?;
                let mut kernel = Worker::start(&kernel_source, &kernel_scratch, EnvironmentLanguage::Python, policy, true).await?;
                let bridge: HostCallHandler = Arc::new(|call| Box::pin(async move {
                    match call.method.as_str() { "environment.observe" => Ok(json!({"validation":true})), _ => bail!("integration not granted during kernel validation") }
                }));
                value(kernel.request("execute", &SeatId::legacy_agent(), json!({"code":"assert env.observe() == {'validation': True}\nvalidation_variable = 42"}), CancellationToken::new(), Some(&bridge)).await?)?;
                kernel.stop().await?;
                let mut restored = Worker::start(&kernel_source, &kernel_scratch, EnvironmentLanguage::Python, policy, true).await?;
                value(restored.request("execute", &SeatId::legacy_agent(), json!({"code":"assert validation_variable == 42"}), CancellationToken::new(), None).await?)?;
                restored.stop().await?;
                fs::remove_dir_all(kernel_scratch)?;
            }
            self.run_checks(&prepared, &scratch, manifest.language, policy).await?;
            Ok(actions)
        }.await;
        let stopped = worker.stop().await;
        fs::remove_dir_all(scratch)?;
        let actions = result?;
        stopped?;
        Ok(actions)
    }
}

pub(super) fn setup_arguments(schema: Option<&Value>, mut arguments: Value) -> Result<Value> {
    if let Some(schema) = schema {
        apply_setup_defaults(schema, &mut arguments);
        jsonschema::validator_for(schema)?
            .validate(&arguments)
            .map_err(|error| anyhow::anyhow!("Invalid environment setup: {error}"))?;
    } else if arguments != json!({}) {
        bail!("this environment does not declare setup parameters");
    }
    Ok(arguments)
}

fn apply_setup_defaults(schema: &Value, arguments: &mut Value) {
    if let (Some(properties), Some(values)) =
        (schema["properties"].as_object(), arguments.as_object_mut())
    {
        for (name, property) in properties {
            if let (false, Some(default)) = (values.contains_key(name), property.get("default")) {
                values.insert(name.clone(), default.clone());
            }
            if let Some(value) = values.get_mut(name) {
                apply_setup_defaults(property, value);
            }
        }
    }
}
