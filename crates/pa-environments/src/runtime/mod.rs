//! Serialized instance ownership, validation, and durable action receipts.
use crate::runner::{
    command_output, docker_command, ensure_image, local_python, sanitized_command, validate_policy,
    NODE_IMAGE, PYTHON_IMAGE,
};
use crate::store::{component, content_hash, copy_sources, read_json, write_json};
use crate::{EnvironmentStore, HostCallHandler, Worker};
use anyhow::{bail, Context, Result};
use pa_types::environments::{
    ActionDescriptor, AgentContext, Definition, EnvironmentLanguage, EnvironmentManifest,
    ExecutionBackend, ExecutionPolicy, HumanInputResult, Instance, InstanceCheckpoint,
    InstanceRoster, OperationReceipt, RoleName, SeatId, ValidationReport, MAX_AGENT_VIEW_BYTES,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod instance;
mod notices;
mod prepare;
mod python;
mod validation;
mod version;
use validation::setup_arguments;
pub use version::{StaleView, ViewGuard};

#[derive(Clone)]
pub struct EnvironmentManager {
    store: EnvironmentStore,
    integrations: Option<HostCallHandler>,
}

enum OpenMode {
    Resume,
    Inspect,
    Initialize(Value),
}

impl EnvironmentManager {
    /// Open an independent environment store and trusted integration adapter.
    /// # Errors
    /// Returns filesystem errors.
    pub fn new(root: PathBuf, integrations: Option<HostCallHandler>) -> Result<Self> {
        Ok(Self {
            store: EnvironmentStore::new(root)?,
            integrations,
        })
    }

    #[must_use]
    pub fn store(&self) -> &EnvironmentStore {
        &self.store
    }

    /// Record a user-requested policy through the UI or builder, outside generated source.
    /// # Errors
    /// Rejects invalid policies or drafts.
    pub fn select_policy(&self, selector: &str, policy: &ExecutionPolicy) -> Result<()> {
        validate_policy(policy)?;
        self.validate_mounts(policy)?;
        let draft = self.store.draft(selector)?;
        let manifest: EnvironmentManifest = read_json(&draft.join("environment.json"))?;
        write_json(
            &self
                .store
                .root()
                .join("selections")
                .join(draft.file_name().context("draft id")?)
                .with_extension("json"),
            &json!({"policy":policy,"manifest":manifest}),
        )
    }

    /// Read the selected execution policy before builder validation.
    /// # Errors
    /// Fails when execution has not been selected through the UI or builder.
    pub fn selected_policy(&self, selector: &str) -> Result<ExecutionPolicy> {
        let draft = self.store.draft(selector)?;
        let selection: Value = read_json(
            &self
                .store
                .root()
                .join("selections")
                .join(draft.file_name().context("draft id")?)
                .with_extension("json"),
        )
        .context("select execution and grants with environments.select_policy or /env policy before validation")?;
        let manifest: EnvironmentManifest = read_json(&draft.join("environment.json"))?;
        if serde_json::to_value(manifest)? != selection["manifest"] {
            bail!(
                "draft capabilities or instructions changed; ask the user for a new policy and apply it with environments.select_policy or /env policy"
            );
        }
        serde_json::from_value(selection["policy"].clone()).map_err(Into::into)
    }

    fn validate_mounts(&self, policy: &ExecutionPolicy) -> Result<()> {
        for mount in &policy.mounts {
            let source = Path::new(&mount.source).canonicalize()?;
            if source.starts_with(self.store.root()) || self.store.root().starts_with(&source) {
                bail!("environment store, engine state, and configuration cannot be mounted");
            }
            if self
                .store
                .root()
                .file_name()
                .is_some_and(|name| name == "environments")
                && self
                    .store
                    .root()
                    .parent()
                    .is_some_and(|root| source.starts_with(root))
            {
                bail!("application configuration and provider credentials cannot be mounted");
            }
        }
        Ok(())
    }

    /// Apply an explicitly reconciled checkpoint without dispatching any actions.
    /// # Errors
    /// Rejects active ownership, missing evidence, mismatched receipts, or unresolved intents.
    // Validate the complete reconciliation before applying any namespace or checkpoint writes.
    #[allow(clippy::too_many_lines)]
    pub fn recover(
        &self,
        id: &str,
        recovery: &pa_types::environments::RecoveryRequest,
    ) -> Result<()> {
        if recovery.evidence.trim().is_empty() {
            bail!("recovery requires evidence from inspected state and external receipts");
        }
        let path = self.store.instance_path(id)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join("owner.lock"))?;
        lease
            .try_lock()
            .context("leave the instance before recovery")?;
        let instance = self.store.instance(id)?;
        let definition = self
            .store
            .definition(&instance.definition_id, Some(&instance.version))?
            .0;
        let checkpoint_path = path.join("checkpoint.json");
        let mut checkpoint: InstanceCheckpoint = if checkpoint_path.exists() {
            read_json(&checkpoint_path)?
        } else {
            // Initialization can fail before the first checkpoint. Only this
            // explicit recovery path may supply its reconciled state and receipt.
            InstanceCheckpoint {
                protocol_version: definition.manifest.protocol_version,
                snapshot: recovery.snapshot.clone(),
                receipts: BTreeMap::new(),
                next_sequence: 1,
                finished: None,
                progress: None,
                progress_sequence: 0,
                requires_recovery: false,
            }
        };
        let python_path = path.join("python-receipts.json");
        let mut python: BTreeMap<String, OperationReceipt> = if python_path.exists() {
            read_json(&python_path)?
        } else {
            BTreeMap::new()
        };
        for (directory, additions, existing) in [
            ("operations", &recovery.receipts, &mut checkpoint.receipts),
            ("python-operations", &recovery.python_receipts, &mut python),
        ] {
            for (operation_id, receipt) in additions {
                component(operation_id)?;
                let intent: Value =
                    read_json(&path.join(directory).join(format!("{operation_id}.json")))?;
                if intent != receipt.request || existing.contains_key(operation_id) {
                    bail!("recovery receipt must match an unresolved intent");
                }
                let mut receipt = receipt.clone();
                if directory == "operations" {
                    receipt.sequence = Some(checkpoint.next_sequence.max(1));
                    checkpoint.next_sequence = checkpoint
                        .next_sequence
                        .max(1)
                        .checked_add(1)
                        .context("operation sequence exhausted")?;
                }
                existing.insert(operation_id.clone(), receipt);
            }
            assert_resolved(&path.join(directory), existing)?;
        }
        if !recovery.python_receipts.is_empty() && !recovery.reset_python {
            bail!("uncertain Python requires an explicit namespace reset");
        }
        if recovery
            .seat_python_receipts
            .keys()
            .any(|seat| !recovery.reset_python_seats.contains(seat))
        {
            bail!("uncertain seat Python requires its explicit namespace reset");
        }
        let mut seat_resets = Vec::new();
        for seat in &recovery.reset_python_seats {
            if instance
                .roster
                .clone()
                .unwrap_or_default()
                .seats
                .iter()
                .all(|entry| {
                    &entry.id != seat
                        || entry.occupant != pa_types::environments::OccupantKind::Agent
                })
            {
                bail!("unknown recovery seat");
            }
            let seat_path = if definition.manifest.protocol_version == 1 {
                path.clone()
            } else {
                path.join("seats").join(seat.as_str())
            };
            let receipts_path = seat_path.join("python-receipts.json");
            let mut receipts: BTreeMap<String, OperationReceipt> = if receipts_path.exists() {
                read_json(&receipts_path)?
            } else {
                BTreeMap::new()
            };
            for (id, receipt) in recovery
                .seat_python_receipts
                .get(seat)
                .into_iter()
                .flatten()
            {
                component(id)?;
                let intent: Value = read_json(
                    &seat_path
                        .join("python-operations")
                        .join(format!("{id}.json")),
                )?;
                if intent != receipt.request || receipts.contains_key(id) {
                    bail!("seat recovery receipt must match an unresolved intent");
                }
                receipts.insert(id.clone(), receipt.clone());
            }
            assert_resolved(&seat_path.join("python-operations"), &receipts)?;
            seat_resets.push((
                receipts_path,
                receipts,
                seat_path.join("participant/.python-state.pkl"),
            ));
        }
        // Validate all reconciled metadata and namespaces before changing files.
        let seats = instance
            .roster
            .unwrap_or_default()
            .seats
            .into_iter()
            .map(|seat| seat.id)
            .collect::<Vec<_>>();
        let mut reconciled = recovery
            .receipts
            .keys()
            .map(|id| &checkpoint.receipts[id])
            .cloned()
            .collect::<Vec<_>>();
        reconciled.sort_by_key(|receipt| receipt.sequence);
        for receipt in reconciled {
            if !receipt.success
                && (!receipt.notices.is_empty()
                    || receipt.finished.is_some()
                    || receipt.progress.is_some())
            {
                bail!("failed recovery receipts cannot publish attention metadata");
            }
            let (_, finished, progress) = notices::response_metadata(
                &json!({"notices":receipt.notices,"finished":receipt.finished,"progress":receipt.progress}),
                &json!({}),
                &checkpoint,
                &seats,
            )?;
            if finished.is_some() {
                checkpoint.finished = finished;
            }
            if progress.is_some() {
                checkpoint.progress = progress;
                checkpoint.progress_sequence = receipt.sequence.unwrap_or_default();
            }
        }
        for (receipts_path, receipts, snapshot) in seat_resets {
            write_json(&receipts_path, &receipts)?;
            if snapshot.exists() {
                fs::remove_file(snapshot)?;
            }
        }
        checkpoint.snapshot = recovery.snapshot.clone();
        checkpoint.requires_recovery = false;
        write_json(
            &path
                .join("recovery")
                .join(format!("{}.json", Uuid::new_v4())),
            recovery,
        )?;
        write_json(&python_path, &python)?;
        if recovery.reset_python {
            let snapshot = path.join("participant/.python-state.pkl");
            if snapshot.exists() {
                fs::remove_file(snapshot)?;
            }
        }
        write_json(&path.join("checkpoint.json"), &checkpoint)
    }

    /// Save only the latest successful validation of unchanged source.
    /// # Errors
    /// Rejects stale or unsuccessful validation.
    pub fn save(&self, selector: &str, id: &str) -> Result<Definition> {
        let draft = self.store.draft(selector)?;
        let latest = self
            .store
            .root()
            .join("validation")
            .join(draft.file_name().context("draft id")?)
            .join("latest.json");
        self.store.save(&draft, id, &read_json(&latest)?)
    }

    /// Create a new independent instance; initialization is performed once.
    /// # Errors
    /// Returns definition, preparation, initialization, and persistence failures.
    pub async fn enter(
        &self,
        id: &str,
        version: Option<&str>,
        policy: ExecutionPolicy,
    ) -> Result<Arc<ActiveInstance>> {
        self.enter_with_arguments(id, version, policy, json!({}))
            .await
    }

    /// Create an instance with schema-validated human setup parameters.
    /// Defaults are applied before validation and recorded in the initialization receipt.
    /// # Errors
    /// Rejects invalid parameters before creating an instance, or returns startup failures.
    #[tracing::instrument(skip(self, policy, arguments))]
    pub async fn enter_with_arguments(
        &self,
        id: &str,
        version: Option<&str>,
        policy: ExecutionPolicy,
        arguments: Value,
    ) -> Result<Arc<ActiveInstance>> {
        let (definition, _) = self.store.definition(id, version)?;
        let roster = crate::default_roster(&definition.manifest)?;
        self.enter_with_roster(id, version, policy, arguments, roster)
            .await
    }

    /// Provision an instance with explicit seats and schema-validated setup.
    /// # Errors
    /// Rejects roster, setup, policy and startup failures.
    #[tracing::instrument(skip(self, policy, arguments, roster))]
    pub async fn enter_with_roster(
        &self,
        id: &str,
        version: Option<&str>,
        policy: ExecutionPolicy,
        arguments: Value,
        roster: InstanceRoster,
    ) -> Result<Arc<ActiveInstance>> {
        let (definition, _) = self.store.definition(id, version)?;
        crate::validate_roster(&definition.manifest, &roster)?;
        let arguments = setup_arguments(definition.manifest.setup_schema.as_ref(), arguments)?;
        let instance = Instance {
            id: Uuid::new_v4().to_string(),
            definition_id: id.into(),
            roster: (definition.manifest.protocol_version == 2
                || roster != InstanceRoster::default())
            .then_some(roster),
            version: definition.version,
            policy,
            created_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis()
                    .try_into()?,
            ),
        };
        let path = self.store.instance_path(&instance.id)?;
        fs::create_dir(&path)?;
        write_json(&path.join("instance.json"), &instance)?;
        self.open(instance, OpenMode::Initialize(arguments)).await
    }

    /// Restore an instance without initialization or automatic action replay.
    /// # Errors
    /// Rejects simultaneous owners and uncertain operations.
    pub async fn resume(&self, id: &str) -> Result<Arc<ActiveInstance>> {
        self.open(self.store.instance(id)?, OpenMode::Resume).await
    }

    /// Restore only the saved engine snapshot for a privileged observation.
    /// Uncertain operations are never replayed and no receipt/checkpoint is changed.
    /// # Errors
    /// Rejects active ownership, missing checkpoints, unknown seats and worker failures.
    #[tracing::instrument(skip_all)]
    pub async fn inspect(&self, id: &str, seat: &SeatId) -> Result<Value> {
        let active = self
            .open(self.store.instance(id)?, OpenMode::Inspect)
            .await?;
        let observation = active.observe(seat).await;
        let disposed = active.leave().await;
        let observation = observation?;
        disposed?;
        Ok(observation)
    }

    // Ownership must span checkpoint verification and both worker startup handshakes.
    #[allow(clippy::too_many_lines)]
    async fn open(&self, instance: Instance, mode: OpenMode) -> Result<Arc<ActiveInstance>> {
        let inspect = matches!(mode, OpenMode::Inspect);
        let resume = !matches!(mode, OpenMode::Initialize(_));
        validate_policy(&instance.policy)?;
        self.validate_mounts(&instance.policy)?;
        let path = self.store.instance_path(&instance.id)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("owner.lock"))?;
        lease
            .try_lock()
            .context("instance is already owned by another application process")?;
        let (definition, source) = self
            .store
            .definition(&instance.definition_id, Some(&instance.version))?;
        let checkpoint: InstanceCheckpoint = if resume {
            read_json(&path.join("checkpoint.json"))
                .context("instance has no recoverable checkpoint")?
        } else {
            InstanceCheckpoint {
                protocol_version: definition.manifest.protocol_version,
                snapshot: json!({}),
                receipts: BTreeMap::new(),
                next_sequence: 1,
                finished: None,
                progress: None,
                progress_sequence: 0,
                requires_recovery: false,
            }
        };
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&checkpoint.protocol_version)
            || checkpoint.protocol_version != definition.manifest.protocol_version
        {
            bail!("unsupported instance checkpoint version");
        }
        if checkpoint.requires_recovery && !inspect {
            bail!("instance requires explicit recovery");
        }
        if resume && !inspect {
            assert_resolved(&path.join("operations"), &checkpoint.receipts)?;
            let receipts_path = path.join("python-receipts.json");
            let receipts = if receipts_path.exists() {
                read_json(&receipts_path)?
            } else {
                BTreeMap::new()
            };
            assert_resolved(&path.join("python-operations"), &receipts)?;
        }
        if instance.policy.python_execution && !definition.manifest.python_execution {
            bail!("Python execution was not declared by this definition");
        }
        let prepared = self
            .prepare(&source, definition.manifest.language, &instance.policy)
            .await?;
        let workspace = path.join("workspace");
        writable_workspace(&workspace)?;
        let mut worker = Worker::start(
            &prepared,
            &workspace,
            definition.manifest.language,
            &instance.policy,
            /*kernel*/ false,
        )
        .await?;
        if resume {
            value(
                worker
                    .request(
                        "restore",
                        &SeatId::legacy_human(),
                        checkpoint.snapshot.clone(),
                        CancellationToken::new(),
                        None,
                    )
                    .await?,
            )?;
        }
        let actions = if resume {
            describe(&mut worker, &definition.manifest).await?
        } else {
            definition.validation.actions.clone()
        };
        let committed_sequence = checkpoint.next_sequence.saturating_sub(1);
        let commits = tokio::sync::watch::channel(committed_sequence).0;
        let active = Arc::new(ActiveInstance {
            instance,
            definition,
            actions,
            path,
            runtime: Mutex::new(InstanceRuntime {
                worker,
                failed: checkpoint.requires_recovery && !inspect,
                checkpoint,
                closed: false,
            }),
            python: Mutex::new(BTreeMap::new()),
            commits,
            inspection: inspect,
            integrations: self.integrations.clone(),
            lease: std::sync::Mutex::new(Some(lease)),
            shutdown: CancellationToken::new(),
        });
        if let OpenMode::Initialize(arguments) = mode {
            active
                .operation(
                    "initialize",
                    &SeatId::legacy_human(),
                    if active.definition.manifest.protocol_version == 2 { json!({"seats":active.roster().seats.iter().map(|seat| json!({"id":seat.id,"role":seat.role})).collect::<Vec<_>>(),"config":arguments}) } else { arguments },
                    "initialization",
                    CancellationToken::new(),
                    ViewGuard::Unbound,
                )
                .await?;
        }
        if !inspect
            && active.definition.manifest.python_execution
            && active.instance.policy.python_execution
        {
            let source = self.kernel_source(&active.instance.policy).await?;
            for seat in active
                .roster()
                .seats
                .iter()
                .filter(|seat| seat.occupant == pa_types::environments::OccupantKind::Agent)
            {
                let scratch = active.python_path(&seat.id).join("participant");
                writable_workspace(&scratch)?;
                active.python.lock().await.insert(
                    seat.id.clone(),
                    Arc::new(Mutex::new(
                        Worker::start(
                            &source,
                            &scratch,
                            EnvironmentLanguage::Python,
                            &active.instance.policy,
                            /*kernel*/ true,
                        )
                        .await?,
                    )),
                );
            }
        }
        Ok(active)
    }
}

struct InstanceRuntime {
    worker: Worker,
    checkpoint: InstanceCheckpoint,
    failed: bool,
    closed: bool,
}

pub struct ActiveInstance {
    pub instance: Instance,
    pub definition: Definition,
    pub actions: Vec<ActionDescriptor>,
    path: PathBuf,
    runtime: Mutex<InstanceRuntime>,
    python: Mutex<BTreeMap<SeatId, Arc<Mutex<Worker>>>>,
    integrations: Option<HostCallHandler>,
    lease: std::sync::Mutex<Option<File>>,
    shutdown: CancellationToken,
    commits: tokio::sync::watch::Sender<u64>,
    inspection: bool,
}

fn value(mut response: Value) -> Result<Value> {
    if response["success"] != true {
        bail!("environment rejected request: {}", response["error"]);
    }
    response
        .as_object_mut()
        .and_then(|fields| fields.remove("result"))
        .context("worker response has no result")
}

fn receipt_value(receipt: &OperationReceipt, request: &Value) -> Result<Value> {
    if &receipt.request != request {
        bail!("operation id already belongs to a different request");
    }
    if !receipt.success {
        bail!("environment rejected operation: {}", receipt.result);
    }
    Ok(receipt.result.clone())
}

async fn describe(
    worker: &mut Worker,
    manifest: &EnvironmentManifest,
) -> Result<Vec<ActionDescriptor>> {
    let actions: Vec<ActionDescriptor> = serde_json::from_value(value(
        worker
            .request(
                "describe_actions",
                &SeatId::legacy_human(),
                json!({}),
                CancellationToken::new(),
                None,
            )
            .await?,
    )?)?;
    let mut names = std::collections::BTreeSet::new();
    for action in &actions {
        component(&action.name)?;
        if !names.insert(&action.name) || action.allowed_roles().is_empty() {
            bail!("actions need unique names and participants");
        }
        match manifest.protocol_version {
            1 => {
                if !action.roles.is_empty()
                    || action.participants.iter().any(|role| {
                        ![RoleName::legacy_human(), RoleName::legacy_agent()].contains(role)
                    })
                {
                    bail!("protocol v1 action participants must be human or agent");
                }
            }
            2 => {
                if !action.participants.is_empty()
                    || action
                        .roles
                        .iter()
                        .any(|name| !manifest.roles.iter().any(|role| &role.name == name))
                {
                    bail!("protocol v2 actions must name declared roles");
                }
            }
            version => bail!("unsupported environment protocol {version}"),
        }
        jsonschema::validator_for(&action.input_schema)?;
        jsonschema::validator_for(&action.output_schema)?;
    }
    Ok(actions)
}

fn intent_once(path: &Path, request: &Value) -> Result<()> {
    fs::create_dir_all(path.parent().context("intent parent")?)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .context("operation has an unresolved intent; explicit recovery is required")?;
    file.write_all(&serde_json::to_vec(request)?)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(path.parent().context("intent parent")?)?.sync_all()?;
    Ok(())
}

fn assert_resolved(directory: &Path, receipts: &BTreeMap<String, OperationReceipt>) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for intent in fs::read_dir(directory)? {
        let path = intent?.path();
        let id = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .context("invalid operation file")?;
        if !receipts.contains_key(id) {
            bail!("operation {id} has an uncertain outcome; inspect instance state and external receipts before explicit recovery");
        }
    }
    Ok(())
}

fn tempfile_path(root: &Path, purpose: &str) -> Result<PathBuf> {
    let path = root
        .join("prepared")
        .join(format!("{purpose}-{}", Uuid::new_v4()));
    writable_workspace(&path)?;
    Ok(path)
}

fn writable_workspace(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The store's parent stays private; the bind-mounted leaf permits the
        // container's non-root UID to write without changing host ownership.
        fs::set_permissions(path, fs::Permissions::from_mode(0o777))?;
    }
    Ok(())
}
