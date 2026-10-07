//! Environment instance responsibility.
use super::{
    assert_resolved, bail, component, intent_once, json, notices, read_json, receipt_value, value,
    write_json, ActiveInstance, Arc, BTreeMap, CancellationToken, Context, HostCallHandler,
    HumanInputResult, InstanceRoster, OperationReceipt, Path, Result, SeatId, StaleView, Value,
    ViewGuard, MAX_AGENT_VIEW_BYTES,
};

impl ActiveInstance {
    #[must_use]
    pub fn roster(&self) -> InstanceRoster {
        self.instance.roster.clone().unwrap_or_default()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The validated setup configuration persisted by this instance's initialization.
    /// # Errors
    /// Returns corrupt checkpoint errors.
    pub fn setup_config(&self) -> Result<Option<Value>> {
        crate::store::initialization_config(&self.path)
    }

    /// Read the participant-specific observation. Identity is bound by the host.
    /// # Errors
    /// Returns worker failures or a recovery-required state.
    pub async fn observe(&self, participant: &SeatId) -> Result<Value> {
        Ok(self.read_view("observe", participant).await?.1)
    }

    /// An observation with the committed-state version it was read at, atomically.
    /// # Errors
    /// Returns worker failures or a recovery-required state.
    pub async fn observe_versioned(&self, participant: &SeatId) -> Result<(u64, Value)> {
        self.read_view("observe", participant).await
    }

    /// Read what an agent seat's model sees each turn in stateless context: the
    /// environment's `agent_view`, or its observation when it defines none.
    /// # Errors
    /// Returns worker failures, a recovery-required state, or an oversized view.
    pub async fn agent_view(&self, participant: &SeatId) -> Result<Value> {
        Ok(self.agent_view_versioned(participant).await?.1)
    }

    /// The `agent_view` with the committed-state version it was read at, atomically:
    /// a mutation bound to this version is rejected once any engine operation commits.
    /// # Errors
    /// Returns worker failures, a recovery-required state, or an oversized view.
    pub async fn agent_view_versioned(&self, participant: &SeatId) -> Result<(u64, Value)> {
        let (version, view) = self.read_view("agent_view", participant).await?;
        check_agent_view(&view)?;
        Ok((version, view))
    }

    /// The committed-state version: it advances with every engine operation
    /// receipt (action, input or initialization) and with nothing else. Python
    /// namespaces, turn accounting and direct messages are not engine state.
    pub async fn version(&self) -> u64 {
        self.runtime.lock().await.checkpoint.next_sequence
    }

    async fn read_view(&self, method: &str, participant: &SeatId) -> Result<(u64, Value)> {
        if !self
            .roster()
            .seats
            .iter()
            .any(|seat| &seat.id == participant)
        {
            bail!("unknown observation seat");
        }
        let mut runtime = self.runtime.lock().await;
        if runtime.failed || runtime.closed {
            bail!("instance is closed or requires recovery");
        }
        let response = runtime
            .worker
            .request(method, participant, json!({}), self.shutdown.clone(), None)
            .await;
        match response {
            Ok(response) => value(response).map(|view| (runtime.checkpoint.next_sequence, view)),
            Err(error) => {
                runtime.worker.cancel().await;
                runtime.failed = true;
                Err(error)
            }
        }
    }

    /// Route human text through the environment, not directly to the model.
    /// # Errors
    /// Returns domain rejection, worker, schema, or persistence failures.
    pub async fn input(&self, text: &str, operation_id: &str) -> Result<HumanInputResult> {
        serde_json::from_value(
            self.operation(
                "handle_input",
                &SeatId::legacy_human(),
                json!({"text":text}),
                operation_id,
                CancellationToken::new(),
                ViewGuard::Unbound,
            )
            .await?,
        )
        .map_err(Into::into)
    }

    /// Route text from a bound seat through the environment's input handler.
    /// # Errors
    /// Rejects unknown seats, invalid responses, or uncertain operations.
    pub async fn input_seat(&self, seat: &SeatId, text: &str, operation_id: &str) -> Result<Value> {
        self.input_seat_guarded(seat, text, operation_id, ViewGuard::Unbound)
            .await
    }

    /// Like [`Self::input_seat`], refused with [`StaleView`] before the environment
    /// runs when `guard` names a version the instance has since committed past.
    /// # Errors
    /// Rejects unknown seats, stale views, invalid responses, or uncertain operations.
    pub async fn input_seat_guarded(
        &self,
        seat: &SeatId,
        text: &str,
        operation_id: &str,
        guard: ViewGuard,
    ) -> Result<Value> {
        if !self.roster().seats.iter().any(|entry| &entry.id == seat) {
            bail!("unknown input seat");
        }
        self.operation(
            "handle_input",
            seat,
            json!({"text":text}),
            operation_id,
            CancellationToken::new(),
            guard,
        )
        .await
    }

    /// Invoke a declared action with host-bound participant identity.
    /// # Errors
    /// Rejects undeclared actions, invalid inputs/outputs, or uncertain operations.
    pub async fn invoke(
        &self,
        participant: &SeatId,
        action: &str,
        arguments: Value,
        operation_id: &str,
        signal: CancellationToken,
    ) -> Result<Value> {
        self.invoke_guarded(
            participant,
            action,
            arguments,
            operation_id,
            signal,
            ViewGuard::Unbound,
        )
        .await
    }

    /// Like [`Self::invoke`], refused with [`StaleView`] before the environment
    /// runs when `guard` names a version the instance has since committed past.
    /// A committed operation ID returns its original receipt regardless.
    /// # Errors
    /// Rejects undeclared actions, stale views, invalid inputs/outputs, or uncertain operations.
    pub async fn invoke_guarded(
        &self,
        participant: &SeatId,
        action: &str,
        arguments: Value,
        operation_id: &str,
        signal: CancellationToken,
        guard: ViewGuard,
    ) -> Result<Value> {
        let role = self
            .roster()
            .seats
            .into_iter()
            .find(|seat| &seat.id == participant)
            .context("unknown acting seat")?
            .role;
        let descriptor = self
            .actions
            .iter()
            .find(|descriptor| {
                descriptor.name == action && descriptor.allowed_roles().contains(&role)
            })
            .context("action is not available to this participant")?;
        jsonschema::validator_for(&descriptor.input_schema)?
            .validate(&arguments)
            .map_err(|error| anyhow::anyhow!("invalid action input: {error}"))?;
        let result = self
            .operation(
                "invoke",
                participant,
                json!({"action":action,"arguments":arguments}),
                operation_id,
                signal,
                guard,
            )
            .await?;
        jsonschema::validator_for(&descriptor.output_schema)?
            .validate(&result)
            .map_err(|error| anyhow::anyhow!("invalid action output: {error}"))?;
        Ok(result)
    }

    // Keep intent dispatch, outcome validation and the atomic commit in one transaction.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn operation(
        &self,
        method: &str,
        participant: &SeatId,
        params: Value,
        id: &str,
        signal: CancellationToken,
        guard: ViewGuard,
    ) -> Result<Value> {
        if self.inspection {
            bail!("inspection cannot dispatch operations");
        }
        component(id)?;
        let mut request = json!({"method":method,"params":params});
        if method != "initialize" || self.definition.manifest.protocol_version == 1 {
            request["participant"] = json!(participant);
        }
        let mut runtime = self.runtime.lock().await;
        if let Some(receipt) = runtime.checkpoint.receipts.get(id) {
            return receipt_value(receipt, &request);
        }
        if runtime.failed || runtime.closed {
            bail!("instance requires recovery or has been left");
        }
        if runtime.checkpoint.finished.is_some() {
            bail!("environment has finished");
        }
        // Checked under the lock that serializes commits, before any intent is written
        // or environment code runs, so a refusal leaves no trace.
        if let ViewGuard::Version(expected) = guard {
            let current = runtime.checkpoint.next_sequence;
            if expected != current {
                return Err(StaleView { expected, current }.into());
            }
        }
        assert_resolved(&self.path.join("operations"), &runtime.checkpoint.receipts)?;
        let intent = self.path.join("operations").join(format!("{id}.json"));
        intent_once(&intent, &request)?;
        let host = self.integration_handler(participant);
        let response = match tokio::select! {
            () = self.shutdown.cancelled() => Err(anyhow::anyhow!("instance is leaving; operation outcome is uncertain")),
            result = runtime.worker.request(method, participant, params, signal, Some(&host)) => result,
        } {
            Ok(response) => response,
            Err(error) => {
                runtime.worker.cancel().await;
                runtime.failed = true;
                runtime.checkpoint.requires_recovery = true;
                if self.path.join("checkpoint.json").exists() {
                    write_json(&self.path.join("checkpoint.json"), &runtime.checkpoint)?;
                }
                return Err(
                    error.context("operation outcome is uncertain; explicit recovery required")
                );
            }
        };
        let snapshot = match runtime
            .worker
            .request(
                "snapshot",
                participant,
                json!({}),
                self.shutdown.clone(),
                None,
            )
            .await
            .and_then(value)
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                runtime.failed = true;
                runtime.checkpoint.requires_recovery = true;
                if self.path.join("checkpoint.json").exists() {
                    write_json(&self.path.join("checkpoint.json"), &runtime.checkpoint)?;
                }
                return Err(
                    error.context("could not checkpoint operation; explicit recovery required")
                );
            }
        };
        let mut receipt = OperationReceipt {
            request: request.clone(),
            success: response["success"] == true,
            result: if response["success"] == true {
                response["result"].clone()
            } else {
                response["error"].clone()
            },
            ..OperationReceipt::default()
        };
        let mut invalid_response = false;
        if method == "invoke" && receipt.success {
            let action = request["params"]["action"]
                .as_str()
                .context("action name")?;
            let descriptor = self
                .actions
                .iter()
                .find(|descriptor| descriptor.name == action)
                .context("action descriptor")?;
            if let Err(error) =
                jsonschema::validator_for(&descriptor.output_schema)?.validate(&receipt.result)
            {
                invalid_response = true;
                receipt.success = false;
                receipt.result = json!(format!("invalid action output: {error}"));
            }
        }
        let metadata = if receipt.success {
            notices::response_metadata(
                &response,
                &request,
                &runtime.checkpoint,
                &self
                    .roster()
                    .seats
                    .into_iter()
                    .map(|seat| seat.id)
                    .collect::<Vec<_>>(),
            )
        } else {
            Ok((Vec::new(), None, None))
        };
        let mut checkpoint = runtime.checkpoint.clone();
        checkpoint.requires_recovery |= invalid_response;
        match metadata {
            Ok((notices, finished, progress)) => {
                receipt.notices = notices;
                receipt.finished = finished;
                receipt.progress = progress;
            }
            Err(error) => {
                receipt.success = false;
                receipt.result = json!(format!("{error:#}; explicit recovery required"));
                checkpoint.requires_recovery = true;
            }
        }
        if !receipt.success && snapshot != checkpoint.snapshot {
            checkpoint.requires_recovery = true;
        }
        let sequence = checkpoint.next_sequence.max(1);
        checkpoint.next_sequence = sequence
            .checked_add(1)
            .context("operation sequence exhausted")?;
        receipt.sequence = Some(sequence);
        if receipt.finished.is_some() {
            checkpoint.finished.clone_from(&receipt.finished);
        }
        if receipt.progress.is_some() {
            checkpoint.progress.clone_from(&receipt.progress);
            checkpoint.progress_sequence = sequence;
        }
        checkpoint.snapshot = snapshot;
        checkpoint.receipts.insert(id.into(), receipt.clone());
        if let Err(error) = write_json(&self.path.join("checkpoint.json"), &checkpoint) {
            runtime.failed = true;
            return Err(error.context("checkpoint write failed; explicit recovery required"));
        }
        runtime.failed = checkpoint.requires_recovery;
        runtime.checkpoint = checkpoint;
        self.commits.send_replace(sequence);
        receipt_value(&receipt, &request)
    }

    pub(super) fn integration_handler(&self, participant: &SeatId) -> HostCallHandler {
        let participant = participant.clone();
        let grants = self.instance.policy.integrations.clone();
        let instance_id = self.instance.id.clone();
        let adapter = self.integrations.clone();
        Arc::new(move |mut call| {
            let participant = participant.clone();
            let adapter = adapter.clone();
            let grants = grants.clone();
            let instance_id = instance_id.clone();
            Box::pin(async move {
                if call.method != "integration.call" {
                    bail!("unrecognized host integration request");
                }
                let name = call
                    .params
                    .get("name")
                    .and_then(Value::as_str)
                    .context("integration name")?;
                if !grants.iter().any(|grant| grant == name) {
                    bail!("integration {name} was not granted to this instance");
                }
                let adapter =
                    adapter.context("selected integration is unavailable on this host")?;
                let params = call
                    .params
                    .as_object_mut()
                    .context("integration parameters must be an object")?;
                params.insert("instance_id".into(), json!(instance_id));
                params.insert("participant".into(), json!(participant));
                adapter(call).await
            })
        })
    }

    /// Checkpoint and stop both processes; no activity survives leaving.
    /// # Errors
    /// Reports uncertain state or failed Python checkpoint after stopping workers.
    pub async fn leave(&self) -> Result<()> {
        self.shutdown.cancel();
        let mut python = self.python.lock().await;
        let mut python_result = Ok(());
        for kernel in python.values_mut() {
            if let Err(error) = kernel.lock().await.stop().await {
                python_result = Err(error);
            }
        }
        python.clear();
        let mut runtime = self.runtime.lock().await;
        if runtime.closed {
            return Ok(());
        }
        let result = runtime.worker.stop().await;
        let legacy_python = if self.definition.manifest.protocol_version == 1 && !self.inspection {
            let receipts_path = self.path.join("python-receipts.json");
            let receipts = if receipts_path.exists() {
                read_json(&receipts_path)?
            } else {
                BTreeMap::new()
            };
            assert_resolved(&self.path.join("python-operations"), &receipts)
        } else {
            Ok(())
        };
        runtime.closed = true;
        self.lease
            .lock()
            .map_err(|_| anyhow::anyhow!("instance ownership lock poisoned"))?
            .take();
        python_result?;
        legacy_python?;
        if runtime.failed {
            bail!("instance stopped with uncertain operations; explicit recovery is required");
        }
        result
    }
}

/// Reject a view too large to resend on every turn.
pub(super) fn check_agent_view(view: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(view)?.len();
    if bytes > MAX_AGENT_VIEW_BYTES {
        bail!(
            "agent_view is {bytes} bytes; summarize history to stay under {MAX_AGENT_VIEW_BYTES}"
        );
    }
    Ok(())
}
