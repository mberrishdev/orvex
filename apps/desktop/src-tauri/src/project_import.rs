//! A committed host checkout is transferred as a Git bundle, never mounted.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) struct ProjectSnapshot {
    directory: PathBuf,
    pub(crate) bundle: PathBuf,
    pub(crate) commit: String,
    pub(crate) branch: String,
    pub(crate) origin: Option<String>,
}

impl ProjectSnapshot {
    pub(crate) fn prepare(project: &Path) -> Result<Self, String> {
        if !project.is_absolute() || !project.is_dir() {
            return Err("projectPath must be an existing absolute host directory.".into());
        }
        let git = crate::find_executable("git")
            .ok_or("Git is required on the host to import a local project.")?;
        let run = |args: &[&str]| -> Result<String, String> {
            let output = Command::new(&git)
                .arg("-C")
                .arg(project)
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(crate::output_error(&output, "snapshot local Git project"));
            }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let commit = run(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let branch = run(&["branch", "--show-current"])?;
        let origin = run(&["remote", "get-url", "origin"])
            .ok()
            .and_then(|url| crate::validate_public_https_repo_url(&url).ok());
        let directory = std::env::temp_dir().join(format!(
            "orvex-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        ));
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let snapshot = Self {
            bundle: directory.join("project.bundle"),
            directory,
            commit,
            branch,
            origin,
        };
        // Pin to the resolved commit so a simultaneous branch change cannot alter the snapshot.
        // Bundle requires a named ref: HEAD is checked before/after bundle creation.
        run(&[
            "bundle",
            "create",
            snapshot.bundle.to_str().ok_or("Non-UTF-8 temporary path")?,
            "HEAD",
        ])?;
        if run(&["rev-parse", "HEAD"])? != snapshot.commit {
            return Err(
                "The project's HEAD changed during import. Retry the workspace creation.".into(),
            );
        }
        Ok(snapshot)
    }
}

impl Drop for ProjectSnapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_contains_committed_checkout_without_dirty_changes_or_credentials() {
        let directory = std::env::temp_dir().join(format!(
            "orvex-project-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .arg("-C")
                .arg(&directory)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Orvex Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(directory.join("example.txt"), "committed").unwrap();
        git(&["add", "example.txt"]);
        git(&["commit", "-m", "fixture"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://secret@example.invalid/private.git",
        ]);
        std::fs::write(directory.join("example.txt"), "dirty").unwrap();
        std::fs::write(directory.join("untracked.txt"), "local secret").unwrap();
        let snapshot = ProjectSnapshot::prepare(&directory).unwrap();
        assert_eq!(snapshot.branch, "main");
        assert!(snapshot.origin.is_none());
        let copy = directory.join("copy");
        let result = Command::new("git")
            .args(["clone", "--"])
            .arg(&snapshot.bundle)
            .arg(&copy)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let result = Command::new("git")
            .arg("-C")
            .arg(&copy)
            .args(["switch", "-c", "orvex/fixture", &snapshot.commit])
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            std::fs::read_to_string(copy.join("example.txt")).unwrap(),
            "committed"
        );
        assert!(!copy.join("untracked.txt").exists());
        let bundle = snapshot.bundle.clone();
        drop(snapshot);
        assert!(!bundle.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
