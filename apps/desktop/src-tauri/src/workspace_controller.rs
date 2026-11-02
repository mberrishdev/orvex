//! Workspace access shared by desktop, CLI, and future MCP transports.
//! Transport adapters supply the application data directory; this controller
//! does not depend on a Tauri window or application handle.
use std::path::PathBuf;

use crate::{Workspace, read_workspaces_at, workspace_slug};

pub(crate) struct WorkspaceController {
    data_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_network_grant_cannot_run_lima_or_create_metadata() {
        let directory = std::env::temp_dir().join(format!("orvex-policy-{}", std::process::id()));
        let controller = WorkspaceController::new(directory.clone());
        let mut called = false;
        let result = controller.set_running("example", true, false, |_, _| {
            called = true;
            Ok(())
        });
        assert!(result.err().unwrap().contains("--network"));
        assert!(!called);
        assert!(
            controller
                .create(
                    "example".into(),
                    Some("https://example.invalid/repo.git".into()),
                    None,
                    false,
                    None,
                    false
                )
                .is_err()
        );
        assert!(!directory.exists());
        assert!(controller.delete("example", false).is_err());
    }
}

impl WorkspaceController {
    pub(crate) fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub(crate) fn list(&self) -> Result<Vec<Workspace>, String> {
        read_workspaces_at(&self.data_dir.join("workspaces.json"))
    }

    pub(crate) fn workspace(&self, id: &str) -> Result<Workspace, String> {
        if workspace_slug(id)? != id {
            return Err("Use the workspace ID exactly as shown by `orvex workspace list`.".into());
        }
        self.list()?
            .into_iter()
            .find(|workspace| workspace.id == id)
            .ok_or_else(|| format!("Workspace `{id}` was not found in Orvex."))
    }
    /// The transport supplies output/progress delivery; lifecycle policy lives here.
    pub(crate) fn set_running(
        &self,
        id: &str,
        running: bool,
        network: bool,
        mut run: impl FnMut(&std::path::Path, &[&str]) -> Result<(), String>,
    ) -> Result<crate::VmStatus, String> {
        if running && !network {
            return Err("Starting a workspace VM enables its configured network. Repeat with `--network` to approve.".into());
        }
        let workspace = self.workspace(id)?;
        let instance = workspace.vm_name.as_deref().ok_or_else(|| {
            "This workspace uses the legacy shared VM. A dedicated workspace VM is required."
                .to_owned()
        })?;
        let home = self.data_dir.join("lima");
        let state = crate::vm_state(&home, instance)?;
        if running && state == "notCreated" {
            return Err(
                "The workspace VM is missing. Recreate the workspace before starting it.".into(),
            );
        }
        if running && state != "running" {
            run(&home, &["start", instance, "--progress", "--yes"])?;
        } else if !running && state == "running" {
            run(&home, &["stop", instance, "--yes"])?;
        }
        Ok(crate::make_vm_status(
            crate::vm_state(&home, instance)?,
            instance,
        ))
    }

    pub(crate) fn lima_home(&self) -> PathBuf {
        self.data_dir.join("lima")
    }

    pub(crate) fn command(
        &self,
        id: &str,
        network: bool,
        command: &[String],
    ) -> Result<std::process::Command, String> {
        crate::workspace_command(self, id, network, command)
    }

    pub(crate) fn create(
        &self,
        name: String,
        repo: Option<String>,
        branch: Option<String>,
        network: bool,
        project: Option<PathBuf>,
        allow_unsupported_platform: bool,
    ) -> Result<crate::WorkspaceCreationResult, String> {
        crate::create_workspace_at(
            &self.data_dir,
            name,
            repo,
            branch,
            network,
            Vec::new(),
            project,
            allow_unsupported_platform,
            |home, args, _| {
                let output = crate::run_limactl(home, args)?;
                if !output.status.success() {
                    return Err(crate::output_error(&output, "create workspace VM"));
                }
                Ok(())
            },
        )
    }

    pub(crate) fn delete(&self, id: &str, confirm: bool) -> Result<(), String> {
        if !confirm {
            return Err("Deletion requires confirm=true and permanently removes the VM disk and all workspace data.".into());
        }
        let _lock = crate::workspace_store::MutationLock::acquire(&self.data_dir)?;
        let workspace = self.workspace(id)?;
        let instance = workspace
            .vm_name
            .as_deref()
            .ok_or("Legacy shared VMs must be deleted from the desktop.")?;
        if crate::vm_state(&self.lima_home(), instance)? != "notCreated" {
            let output =
                crate::run_limactl(&self.lima_home(), &["delete", "--force", instance, "--yes"])?;
            if !output.status.success() {
                return Err(crate::output_error(&output, "delete workspace VM"));
            }
        }
        let mut records = self.list()?;
        records.retain(|workspace| workspace.id != id);
        crate::write_workspaces_at(&self.data_dir.join("workspaces.json"), &records)
    }
}
