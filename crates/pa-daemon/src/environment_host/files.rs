//! Atomic environment metadata and capability checks.
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{fs::OpenOptions, io::Write, path::Path};

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .map_err(Into::into)
}
pub(crate) fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("metadata parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub(crate) fn authenticate(expected: &str, supplied: &str) -> Result<()> {
    let mut difference = expected.len() ^ supplied.len();
    for (left, right) in expected.bytes().zip(supplied.bytes()) {
        difference |= usize::from(left ^ right);
    }
    if expected.len() < 32 || difference != 0 {
        bail!("invalid environment capability");
    }
    Ok(())
}

pub(crate) fn inbox_lock(path: &Path) -> Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    file.lock()?;
    Ok(file)
}
