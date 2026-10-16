//! Content-addressed definitions and atomic, instance-local checkpoints.
use anyhow::{bail, Context, Result};
use pa_types::environments::{
    Definition, EnvironmentLanguage, EnvironmentManifest, ExecutionPolicy, Instance,
    InstanceCheckpoint, ValidationReport, DRAFT_PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct EnvironmentStore {
    root: PathBuf,
}

impl EnvironmentStore {
    /// Open the environment store without reading ordinary Prime settings.
    /// # Errors
    /// Returns filesystem errors when the directories cannot be created.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
        let root = root.canonicalize()?;
        for directory in ["drafts", "definitions", "instances", "prepared"] {
            fs::create_dir_all(root.join(directory))?;
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a draft including the language SDK and an executable example.
    /// # Errors
    /// Returns filesystem errors. Creation never executes generated code.
    pub fn create_draft(
        &self,
        description: &str,
        language: EnvironmentLanguage,
        python_execution: bool,
        policy: &ExecutionPolicy,
    ) -> Result<PathBuf> {
        let path = self.root.join("drafts").join(Uuid::new_v4().to_string());
        fs::create_dir(&path)?;
        let manifest = EnvironmentManifest {
            protocol_version: DRAFT_PROTOCOL_VERSION,
            name: "New environment".into(),
            purpose: description.into(),
            language,
            instructions: description.into(),
            setup_schema: None,
            roles: serde_json::from_value(serde_json::json!([
                {"name":"human","occupants":["human"]}, {"name":"agent","occupants":["agent"]}
            ]))?,
            direct_messages: None,
            agent_context: pa_types::environments::AgentContext::default(),
            missed_turns: false,
            python_execution,
            recommended_backend: if python_execution {
                pa_types::environments::ExecutionBackend::Docker
            } else {
                policy.backend
            },
        };
        write_json(&path.join("environment.json"), &manifest)?;
        write_json(&path.join("policy.json"), policy)?;
        match language {
            EnvironmentLanguage::Python => {
                fs::write(path.join("sdk.py"), include_str!("../sdk/python/sdk.py"))?;
                fs::write(
                    path.join("runner.py"),
                    include_str!("../sdk/python/runner.py"),
                )?;
                fs::write(
                    path.join("environment.py"),
                    include_str!("../fixtures/counter_seats/environment.py"),
                )?;
                fs::write(
                    path.join("checks.py"),
                    include_str!("../fixtures/counter/checks.py"),
                )?;
                fs::write(
                    path.join("requirements.lock"),
                    "# Add exact versions and hashes for dependencies.\n",
                )?;
            }
            EnvironmentLanguage::TypeScript => {
                fs::write(
                    path.join("sdk.ts"),
                    include_str!("../sdk/typescript/sdk.ts"),
                )?;
                fs::write(
                    path.join("runner.ts"),
                    include_str!("../sdk/typescript/runner.ts"),
                )?;
                fs::write(
                    path.join("environment.ts"),
                    include_str!("../fixtures/counter_seats/environment.ts"),
                )?;
                fs::write(
                    path.join("checks.ts"),
                    include_str!("../fixtures/counter/checks.ts"),
                )?;
                fs::write(
                    path.join("package.json"),
                    "{\"name\":\"environment\",\"version\":\"1.0.0\",\"type\":\"module\"}\n",
                )?;
                fs::write(path.join("package-lock.json"), "{\"name\":\"environment\",\"version\":\"1.0.0\",\"lockfileVersion\":3,\"packages\":{\"\":{\"name\":\"environment\",\"version\":\"1.0.0\"}}}\n")?;
            }
        }
        Ok(path)
    }

    /// Resolve a caller-supplied draft beneath this store.
    /// # Errors
    /// Rejects traversal, symlinks, and paths outside the drafts directory.
    pub fn draft(&self, selector: &str) -> Result<PathBuf> {
        let path = if Path::new(selector).is_absolute() {
            PathBuf::from(selector)
        } else {
            self.root.join("drafts").join(component(selector)?)
        };
        let path = path.canonicalize()?;
        let parent = path.parent().context("draft has no parent")?;
        if parent != self.root.join("drafts") {
            bail!("draft must be an immediate child of this store's drafts directory");
        }
        source_files(&path)?;
        Ok(path)
    }

    /// Save exactly the validated source as an immutable content-addressed version.
    /// # Errors
    /// Rejects failed/stale validation or an invalid definition identifier.
    pub fn save(&self, draft: &Path, id: &str, report: &ValidationReport) -> Result<Definition> {
        component(id)?;
        if !report.passed || content_hash(draft)? != report.content_hash {
            bail!("save requires successful validation of the current draft content");
        }
        let manifest: EnvironmentManifest = read_json(&draft.join("environment.json"))?;
        let definition = Definition {
            id: id.into(),
            version: report.content_hash.clone(),
            manifest,
            validation: report.clone(),
        };
        let parent = self.root.join("definitions").join(id);
        fs::create_dir_all(&parent)?;
        let destination = parent.join(&definition.version);
        if destination.exists() {
            let existing: Definition = read_json(&destination.join("definition.json"))?;
            if existing.manifest != definition.manifest || existing.version != definition.version {
                bail!("definition version already exists with different metadata");
            }
            return Ok(existing);
        }
        let staging = parent.join(format!(".{}", Uuid::new_v4()));
        fs::create_dir(&staging)?;
        copy_sources(draft, &staging.join("source"))?;
        // Validate the copied bytes as well: a concurrent builder edit cannot publish
        // content that was never checked.
        if content_hash(&staging.join("source"))? != report.content_hash {
            fs::remove_dir_all(&staging)?;
            bail!("draft changed while saving");
        }
        write_json(&staging.join("definition.json"), &definition)?;
        fs::rename(&staging, &destination)?;
        sync_directory(&parent)?;
        Ok(definition)
    }

    /// Read a definition and verify its source integrity before execution.
    /// # Errors
    /// Rejects missing definitions or changed immutable source files.
    pub fn definition(&self, id: &str, version: Option<&str>) -> Result<(Definition, PathBuf)> {
        let parent = self.root.join("definitions").join(component(id)?);
        let version = if let Some(version) = version {
            component(version)?.to_owned()
        } else {
            let mut versions: Vec<_> = fs::read_dir(&parent)?
                .filter_map(std::result::Result::ok)
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .collect();
            versions.sort_by_key(|entry| {
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
            });
            versions
                .last()
                .context("definition has no saved versions")?
                .file_name()
                .to_string_lossy()
                .into_owned()
        };
        let path = parent.join(version);
        let definition: Definition = read_json(&path.join("definition.json"))?;
        let source = path.join("source");
        if content_hash(&source)? != definition.version {
            bail!("saved definition failed its integrity check");
        }
        Ok((definition, source))
    }

    /// List every saved version; failed drafts are not definitions.
    /// # Errors
    /// Returns corrupt catalog metadata and filesystem errors.
    pub fn list(&self) -> Result<Vec<Definition>> {
        let mut definitions = Vec::new();
        for entry in fs::read_dir(self.root.join("definitions"))? {
            let entry = entry?;
            // Finder and other tools drop stray files (.DS_Store) beside definitions.
            if !entry.file_type()?.is_dir() {
                continue;
            }
            for version in fs::read_dir(entry.path())? {
                let version = version?;
                if !version.file_name().to_string_lossy().starts_with('.') {
                    definitions.push(read_json(&version.path().join("definition.json"))?);
                }
            }
        }
        definitions.sort_by(|left: &Definition, right| {
            (&left.id, &left.version).cmp(&(&right.id, &right.version))
        });
        Ok(definitions)
    }

    /// List saved instances without starting workers, newest first, then by ID.
    /// # Errors
    /// Returns filesystem errors and corrupt instance metadata.
    pub fn list_instances(&self) -> Result<Vec<Instance>> {
        let mut instances = Vec::new();
        for entry in fs::read_dir(self.root.join("instances"))? {
            let entry = entry?;
            // Participant transcripts live inside instance directories. The
            // session engine places its shared artifacts beside those directories.
            if entry.file_name() == "session-artifacts" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                instances.push(self.instance(&entry.file_name().to_string_lossy())?);
            }
        }
        instances.sort_by(|left: &Instance, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(instances)
    }

    /// Read instance metadata without starting it.
    /// # Errors
    /// Returns invalid identifiers and corrupt/missing metadata.
    pub fn instance(&self, id: &str) -> Result<Instance> {
        let path = self.instance_path(id)?.join("instance.json");
        let mut instance: Instance = read_json(&path)?;
        if instance.created_at.is_none() {
            let metadata = fs::metadata(path)?;
            let created = metadata.created().or_else(|_| metadata.modified())?;
            instance.created_at = Some(
                created
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis()
                    .try_into()?,
            );
        }
        Ok(instance)
    }

    /// Read the validated setup configuration a protocol-2 instance was initialized with.
    /// It comes from the persisted initialization receipt, never from engine state.
    /// # Errors
    /// Returns invalid identifiers and corrupt checkpoints.
    pub fn instance_config(&self, id: &str) -> Result<Option<Value>> {
        initialization_config(&self.instance_path(id)?)
    }

    /// Every committed engine snapshot of an instance, ordered by operation
    /// sequence; sequences recorded before snapshot history existed are absent.
    /// # Errors
    /// Returns invalid identifiers and corrupt checkpoints or history rows.
    pub fn snapshot_history(&self, id: &str) -> Result<Vec<(u64, Value)>> {
        crate::snapshot_history::read(&self.instance_path(id)?)
    }

    /// Resolve the durable directory for an instance.
    /// # Errors
    /// Rejects path traversal.
    pub fn instance_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.root.join("instances").join(component(id)?))
    }
}

/// The `config` argument of the persisted initialization request in an instance directory.
pub(crate) fn initialization_config(instance: &Path) -> Result<Option<Value>> {
    let path = instance.join("checkpoint.json");
    if !path.exists() {
        return Ok(None);
    }
    let checkpoint: InstanceCheckpoint = read_json(&path)?;
    Ok(checkpoint
        .receipts
        .get("initialization")
        .filter(|_| checkpoint.protocol_version == 2)
        .and_then(|receipt| receipt.request["params"].get("config"))
        .filter(|config| config.as_object().is_none_or(|fields| !fields.is_empty()))
        .cloned())
}

pub(crate) fn component(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 128
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("invalid identifier: use letters, digits, hyphens, and underscores");
    }
    Ok(value)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("decode {}", path.display()))
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("file needs a parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    sync_directory(parent)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all().map_err(Into::into)
}
#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if [
                "node_modules",
                ".venv",
                ".python-deps",
                "__pycache__",
                ".git",
                "policy.json",
            ]
            .contains(&entry.file_name().to_string_lossy().as_ref())
            {
                continue;
            }
            if kind.is_symlink() {
                bail!("environment sources may not contain symlinks");
            }
            if kind.is_dir() {
                directories.push(entry.path());
            } else if kind.is_file() {
                if entry.metadata()?.len() > 16 * 1024 * 1024 {
                    bail!("source file exceeds 16 MiB");
                }
                files.push(entry.path().strip_prefix(root)?.to_owned());
                if files.len() > 4096 {
                    bail!("environment contains more than 4096 source files");
                }
            } else {
                bail!("environment source must contain ordinary files");
            }
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn content_hash(root: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    for path in source_files(root)? {
        let name = path.to_string_lossy();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        let bytes = fs::read(root.join(path))?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn copy_sources(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for path in source_files(source)? {
        let target = destination.join(&path);
        fs::create_dir_all(target.parent().context("source file parent")?)?;
        fs::copy(source.join(path), target)?;
    }
    Ok(())
}
