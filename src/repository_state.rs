//! Shared private repository namespace for task records and handoff memory.
mod storage;
pub(crate) use storage::Directory;

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Identity {
    schema: u32,
    common_dir: PathBuf,
}

pub(crate) struct RepositoryState {
    pub state_root: PathBuf,
    pub relative: PathBuf,
    pub common: PathBuf,
}

fn io<T>(value: std::io::Result<T>) -> Result<T> {
    value.map_err(|e| Error::msg(format!("Repository state: {e}")))
}

impl RepositoryState {
    pub fn locate(root: &Path) -> Result<Self> {
        let common = crate::git::canonical_common_dir(root)?;
        let digest = crate::hash::sha256_hex(common.as_os_str().as_bytes());
        Ok(Self {
            state_root: io(Directory::canonical_root(&crate::paths::state_dir()?))?,
            relative: Path::new("repositories").join(digest),
            common,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.state_root.join(&self.relative)
    }

    pub fn directory(&self, writing: bool) -> Result<Option<Directory>> {
        let Some(directory) = io(Directory::open(&self.state_root, &self.relative, writing))?
        else {
            return Ok(None);
        };
        let expected = Identity {
            schema: 1,
            common_dir: self.common.clone(),
        };
        if writing {
            let bytes = serde_json::to_vec(&expected)
                .map_err(|_| Error::msg("Cannot encode repository.json"))?;
            io(directory.publish("repository.json", &bytes))?;
        }
        let bytes = io(directory.read("repository.json"))?
            .ok_or_else(|| Error::msg("Repository state: missing repository.json"))?;
        let stored: Identity = serde_json::from_slice(&bytes)
            .map_err(|_| Error::msg("Repository state: invalid repository.json"))?;
        if stored != expected {
            return Err(Error::msg(
                "Repository state repository.json commonDir mismatch; refusing to reuse repository state",
            ));
        }
        Ok(Some(directory))
    }
}
