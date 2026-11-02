use std::path::{Path, PathBuf};

/// Serialize read-modify-write operations across UI, CLI and MCP processes.
/// Readers use atomic metadata snapshots and do not need to hold this lock.
pub(crate) struct MutationLock(PathBuf);

impl MutationLock {
    pub(crate) fn acquire(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let path = directory.join("workspace-mutation.lock");
        std::fs::create_dir(&path).map_err(|e| format!(
            "Workspace metadata is busy ({e}). Retry when the other operation finishes. If Orvex crashed, verify no workspace mutation is running before removing {}.", path.display()))?;
        let guard = Self(path);
        std::fs::write(guard.0.join("owner"), std::process::id().to_string())
            .map_err(|e| e.to_string())?;
        Ok(guard)
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Failed creation cannot leave an unrecorded app-owned VM consuming resources.
pub(crate) struct VmCreationGuard {
    pub(crate) home: PathBuf,
    pub(crate) instance: String,
    pub(crate) committed: bool,
}

impl Drop for VmCreationGuard {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) =
                crate::run_limactl(&self.home, &["delete", "--force", &self.instance, "--yes"])
            {
                eprintln!(
                    "Could not clean up incomplete workspace VM {}: {error}",
                    self.instance
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutation_locks_exclude_other_writers_and_release_on_drop() {
        let path = std::env::temp_dir().join(format!("orvex-lock-test-{}", std::process::id()));
        let guard = MutationLock::acquire(&path).unwrap();
        assert!(MutationLock::acquire(&path).is_err());
        drop(guard);
        drop(MutationLock::acquire(&path).unwrap());
        std::fs::remove_dir_all(path).unwrap();
    }
}
