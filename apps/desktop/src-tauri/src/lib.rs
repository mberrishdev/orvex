mod mcp_connections;
mod environment_plan;
mod project_configuration;
mod project_requirements;
mod workspace_store;
mod project_import;
mod mcp;
mod command_sessions;
mod workspace_controller;

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    platform: &'static str,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    id: String,
    name: String,
    branch: String,
    #[serde(default)]
    base_branch: Option<String>,
    status: String,
    path: String,
    #[serde(default)]
    repo_url: Option<String>,
    // Missing on older records; those workspaces remain on the legacy shared VM.
    #[serde(default)]
    vm_name: Option<String>,
    #[serde(default)]
    agent_providers: Vec<String>,
    #[serde(default)]
    compose_file: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentInstallResult {
    provider_id: String,
    provider_name: String,
    installed: bool,
    version: Option<String>,
    message: String,
    login_hint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceCreationResult {
    workspace: Workspace,
    agent_installations: Vec<AgentInstallResult>,
    setup_commands: Vec<String>,
    setup_error: Option<String>,
    setup_result: Option<ProjectSetupResult>,
    requirements: Option<project_requirements::Requirements>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceServices {
    workspace_id: String,
    compose_file: Option<String>,
    docker_installed: bool,
    vm_ip: Option<String>,
    output: String,
    containers: Vec<ComposeContainer>,
    ports: Vec<WorkspacePort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ComposePsLine {
    service: String,
    state: String,
    #[serde(default)]
    health: String,
    #[serde(default)]
    publishers: Option<Vec<ComposePublisher>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ComposePublisher {
    #[serde(rename = "URL")]
    url: String,
    target_port: u16,
    published_port: Option<u16>,
    protocol: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ComposeContainer {
    service: String,
    state: String,
    health: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspacePort {
    service: String,
    target_port: u16,
    published_port: u16,
    protocol: String,
    url: Option<String>,
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProjectSetupManifest {
    version: u32,
    #[serde(default)]
    setup: ProjectSetup,
    #[serde(default)]
    services: Option<ProjectServices>,
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProjectSetup {
    commands: Vec<String>,
}

impl Default for ProjectSetup {
    fn default() -> Self {
        Self { commands: Vec::new() }
    }
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProjectServices {
    compose_file: Option<String>,
}

struct ProjectConfig {
    setup_commands: Vec<String>,
    compose_file: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSetupResult {
    commands: Vec<String>,
    completed: usize,
    failed_command: Option<String>,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LimaStatus {
    installed: bool,
    version: Option<String>,
    install_supported: bool,
    installer: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VmStatus {
    state: String,
    instance_name: String,
    image_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VmProgress {
    phase: &'static str,
    message: String,
    downloaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentProvider {
    id: &'static str,
    name: &'static str,
    executable: &'static str,
    login_hint: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentPreflight {
    workspace_id: String,
    provider_id: String,
    provider_name: String,
    installed: bool,
    version: Option<String>,
    message: String,
    login_hint: String,
}

const AGENT_PROVIDERS: [AgentProvider; 6] = [
    AgentProvider {
        id: "codex",
        name: "OpenAI Codex",
        executable: "codex",
        login_hint: "Run `codex login` in the guest terminal.",
    },
    AgentProvider {
        id: "copilot",
        name: "GitHub Copilot CLI",
        executable: "copilot",
        login_hint: "Run `copilot` in the guest terminal and follow its sign-in prompt.",
    },
    AgentProvider {
        id: "claude",
        name: "Claude Code",
        executable: "claude",
        login_hint: "Run `claude auth login` in the guest terminal.",
    },
    AgentProvider {
        id: "pi",
        name: "Pi Coding Agent",
        executable: "pi",
        login_hint: "Run `pi` in the guest terminal and configure a model provider.",
    },
    AgentProvider {
        id: "cursor",
        name: "Cursor Agent CLI",
        executable: "agent",
        login_hint: "Run `agent login` in the guest terminal.",
    },
    AgentProvider {
        id: "gemini",
        name: "Gemini CLI",
        executable: "gemini",
        login_hint: "Run `gemini` in the guest terminal and follow its sign-in setup.",
    },
];

const POC_INSTANCE: &str = "orvex-poc";

fn find_executable(name: &str) -> Option<PathBuf> {
    let executable_name = if cfg!(target_os = "windows") { format!("{name}.exe") } else { name.to_owned() };
    let candidates = if name == "limactl" {
        vec![
            PathBuf::from("/opt/homebrew/bin/limactl"),
            PathBuf::from("/usr/local/bin/limactl"),
            PathBuf::from("/opt/local/bin/limactl"),
        ]
    } else if name == "brew" {
        vec![
            PathBuf::from("/opt/homebrew/bin/brew"),
            PathBuf::from("/usr/local/bin/brew"),
        ]
    } else {
        vec![PathBuf::from("/opt/homebrew/bin").join(name), PathBuf::from("/usr/local/bin").join(name), PathBuf::from("/usr/bin").join(name)]
    };
    candidates
        .into_iter()
        .chain(std::env::var_os("PATH").into_iter().flat_map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(&executable_name))
                .collect::<Vec<_>>()
        }))
        .find(|path| path.is_file())
}

fn inspect_lima() -> LimaStatus {
    let executable = find_executable("limactl");
    let version = executable.as_ref().and_then(|path| {
        Command::new(path)
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    });
    LimaStatus {
        installed: version.is_some(),
        version,
        install_supported: cfg!(target_os = "macos") && find_executable("brew").is_some(),
        installer: find_executable("brew").map(|path| path.display().to_string()),
    }
}

fn install_lima_blocking() -> Result<LimaStatus, String> {
    if !cfg!(target_os = "macos") {
        return Err("Lima installation from Orvex is currently supported on macOS only.".into());
    }
    if inspect_lima().installed {
        return Ok(inspect_lima());
    }
    let brew = find_executable("brew").ok_or_else(|| {
        "Homebrew was not found. Install Homebrew, then retry from Orvex.".to_owned()
    })?;
    let output = Command::new(brew)
        .args(["install", "lima"])
        .output()
        .map_err(|error| format!("Could not start Homebrew: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let details = if stderr.is_empty() { stdout } else { stderr };
        return Err(if details.is_empty() {
            format!("Homebrew exited with {}.", output.status)
        } else {
            format!("Lima installation failed: {details}")
        });
    }
    let status = inspect_lima();
    if !status.installed {
        return Err(
            "Homebrew finished, but Orvex could not find limactl. Restart Orvex and check again."
                .into(),
        );
    }
    Ok(status)
}

fn app_lima_home(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?
        .join("lima");
    std::fs::create_dir_all(&home)
        .map_err(|error| format!("Could not prepare Orvex Lima data: {error}"))?;
    Ok(home)
}

fn workspace_store_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("workspaces.json"))
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))
}

fn read_workspaces(app: &AppHandle) -> Result<Vec<Workspace>, String> {
    let directory = app.path().app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?;
    workspace_controller::WorkspaceController::new(directory).list()
}

fn read_workspaces_at(path: &Path) -> Result<Vec<Workspace>, String> {
    match std::fs::read(&path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|error| format!("Could not read workspace list: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read workspace list: {error}")),
    }
}

fn write_workspaces(app: &AppHandle, workspaces: &[Workspace]) -> Result<(), String> {
    write_workspaces_at(&workspace_store_path(app)?, workspaces)
}

fn write_workspaces_at(path: &Path, workspaces: &[Workspace]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not locate Orvex app data directory.".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare Orvex app data: {error}"))?;
    let data = serde_json::to_vec_pretty(workspaces)
        .map_err(|error| format!("Could not encode workspace list: {error}"))?;
    let temporary = parent.join(format!(".workspaces-{}-{}.tmp", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos()));
    std::fs::write(&temporary, data).map_err(|error| format!("Could not save workspace list: {error}"))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Could not replace workspace list: {error}"));
    }
    Ok(())
}

fn workspace_slug(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a workspace name.".into());
    }
    if name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("Use a name of up to 64 characters without control characters.".into());
    }

    let mut slug = String::new();
    let mut previous_dash = true;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return Err("Use at least one English letter or number in the workspace name.".into());
    }
    Ok(slug)
}

fn validate_public_https_repo_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let Some(rest) = value.strip_prefix("https://") else {
        return Err("Repository import currently supports public HTTPS URLs only.".into());
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let has_path = rest.contains('/') && !rest.ends_with('/');
    if value.len() > 2048
        || value.chars().any(char::is_whitespace)
        || value.contains(['?', '#'])
        || authority.is_empty()
        || authority.contains('@')
        || !authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        || !has_path
    {
        return Err("Enter a public HTTPS Git URL without credentials or query parameters.".into());
    }
    Ok(value.to_owned())
}

fn run_limactl(home: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let executable = find_executable("limactl")
        .ok_or_else(|| "Lima is not installed. Install it before starting a VM.".to_owned())?;
    Command::new(executable)
        .env("LIMA_HOME", home)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run Lima: {error}"))
}

fn output_error(output: &std::process::Output, action: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let details = if stderr.is_empty() { stdout } else { stderr };
    if details.is_empty() {
        format!("Could not {action}: Lima exited with {}.", output.status)
    } else {
        format!("Could not {action}: {details}")
    }
}

fn send_stream_lines<R: Read + Send + 'static>(stream: R, sender: Sender<String>) {
    for line in BufReader::new(stream).lines() {
        match line {
            Ok(line) => {
                if sender.send(line).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(format!("Could not read Lima output: {error}"));
                break;
            }
        }
    }
}

fn map_lima_progress(line: &str, workspace_id: Option<&str>) -> Option<VmProgress> {
    let line = line.trim();
    let lower = line.to_ascii_lowercase();
    let (phase, message) = if lower.contains("attempting to download the image")
        || lower.contains("downloading the image")
    {
        ("downloading", "Downloading the Ubuntu Linux image…")
    } else if lower.contains("downloaded the image") {
        ("preparing", "Image downloaded. Preparing the virtual disk…")
    } else if lower.contains("converting qcow2") || lower.contains("converting `") {
        (
            "preparing",
            "Converting the image for Apple Virtualization…",
        )
    } else if lower.contains("resize instance") {
        ("preparing", "Allocating the VM disk…")
    } else if lower.contains("starting the instance") || lower.contains("starting vz") {
        ("booting", "Starting the Linux virtual machine…")
    } else if lower.contains("waiting for the guest agent")
        || lower.contains("cloud-init provisioning started")
        || lower.contains("cloud-init monitoring")
    {
        ("provisioning", "Ubuntu is completing its first boot setup…")
    } else if lower.contains("ready. run") {
        ("ready", "Linux VM is ready.")
    } else {
        return None;
    };
    Some(VmProgress {
        phase,
        message: message.to_owned(),
        downloaded_bytes: None,
        workspace_id: workspace_id.map(str::to_owned),
    })
}

fn lima_download_temp(pid: u32) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let cache = PathBuf::from(home).join("Library/Caches/lima/download/by-url-sha256");
    let prefix = format!("data.tmp.{pid}.");
    for directory in std::fs::read_dir(cache).ok()? {
        let directory = directory.ok()?;
        for file in std::fs::read_dir(directory.path()).ok()? {
            let file = file.ok()?;
            if file.file_name().to_string_lossy().starts_with(&prefix) {
                return Some(file.path());
            }
        }
    }
    None
}

fn monitor_lima_download(
    app: AppHandle,
    pid: u32,
    stop: Arc<AtomicBool>,
    workspace_id: Option<String>,
) {
    let mut last_bytes = None;
    let mut temp_path = None;
    while !stop.load(Ordering::Relaxed) {
        if temp_path.is_none() {
            temp_path = lima_download_temp(pid);
        }
        if let Some(bytes) = temp_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok().map(|metadata| metadata.len()))
        {
            if last_bytes != Some(bytes) {
                let _ = app.emit(
                    "vm-start-progress",
                    VmProgress {
                        phase: "downloading",
                        message: "Downloading the Ubuntu Linux image…".into(),
                        downloaded_bytes: Some(bytes),
                        workspace_id: workspace_id.clone(),
                    },
                );
                last_bytes = Some(bytes);
            }
        } else if temp_path.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(750));
    }
}

fn run_limactl_with_progress(
    app: &AppHandle,
    home: &PathBuf,
    args: &[&str],
    workspace_id: Option<&str>,
) -> Result<(), String> {
    let executable = find_executable("limactl")
        .ok_or_else(|| "Lima is not installed. Install it before starting a VM.".to_owned())?;
    let mut child = Command::new(executable)
        .env("LIMA_HOME", home)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start Lima: {error}"))?;
    let stop_monitor = Arc::new(AtomicBool::new(false));
    let download_monitor = {
        let stop = Arc::clone(&stop_monitor);
        let app = app.clone();
        let pid = child.id();
        let workspace_id = workspace_id.map(str::to_owned);
        thread::spawn(move || monitor_lima_download(app, pid, stop, workspace_id))
    };
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = {
        let sender = sender.clone();
        thread::spawn(move || send_stream_lines(stdout, sender))
    };
    let stderr_reader = thread::spawn(move || send_stream_lines(stderr, sender));
    let mut recent_output = VecDeque::with_capacity(40);
    for line in receiver {
        if let Some(progress) = map_lima_progress(&line, workspace_id) {
            let _ = app.emit("vm-start-progress", progress);
        }
        if recent_output.len() == 40 {
            recent_output.pop_front();
        }
        recent_output.push_back(line);
    }
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    stop_monitor.store(true, Ordering::Relaxed);
    let _ = download_monitor.join();
    let status = child
        .wait()
        .map_err(|error| format!("Could not wait for Lima: {error}"))?;
    if !status.success() {
        let details = recent_output
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(if details.is_empty() {
            format!("Could not start the Orvex VM: Lima exited with {status}.")
        } else {
            format!("Could not start the Orvex VM: {details}")
        });
    }
    Ok(())
}

fn vm_state(home: &Path, instance_name: &str) -> Result<String, String> {
    let output = run_limactl(home, &["list", "--json"])?;
    if !output.status.success() {
        return Err(output_error(&output, "read VM status"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let instance: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("Could not parse Lima status: {error}"))?;
        if instance.get("name").and_then(|name| name.as_str()) == Some(instance_name) {
            return Ok(
                match instance.get("status").and_then(|status| status.as_str()) {
                    Some("Running") => "running".to_owned(),
                    Some("Stopped") => "stopped".to_owned(),
                    Some(other) => other.to_lowercase(),
                    None => "unknown".to_owned(),
                },
            );
        }
    }
    Ok("notCreated".to_owned())
}

fn guest_user(home: &Path, instance_name: &str) -> Result<(String, PathBuf, bool), String> {
    let output = run_limactl(&home.to_path_buf(), &["list", "--json"])?;
    if !output.status.success() {
        return Err(output_error(&output, "read Linux user configuration"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let instance: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("Could not parse Lima instance configuration: {error}"))?;
        if instance.get("name").and_then(|name| name.as_str()) != Some(instance_name) {
            continue;
        }
        let user = instance
            .get("config")
            .and_then(|config| config.get("user"))
            .ok_or_else(|| "Lima does not report the guest user configuration.".to_owned())?;
        let name = user
            .get("name")
            .and_then(|name| name.as_str())
            .ok_or_else(|| "Lima does not report the guest user name.".to_owned())?;
        let guest_home = user
            .get("home")
            .and_then(|path| path.as_str())
            .ok_or_else(|| "Lima does not report the guest home directory.".to_owned())?;
        let passwordless_sudo = user
            .get("passwordlessSudo")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        return Ok((
            name.to_owned(),
            PathBuf::from(guest_home),
            passwordless_sudo,
        ));
    }
    Err("The Orvex Linux VM has not been created.".into())
}

fn guest_identity_id(home: &Path, instance_name: &str, flag: &str) -> Result<u32, String> {
    let output = run_limactl(home, &["shell", instance_name, "id", flag])?;
    if !output.status.success() {
        return Err(output_error(&output, "read the Linux guest identity"));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|_| "The Linux guest returned an invalid user or group ID.".to_owned())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn open_workspace_terminal_blocking(
    app: &AppHandle,
    workspace_id: String,
    network_enabled: bool,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "Opening a Lima workspace terminal is currently supported on macOS only.".into(),
        );
    }
    let slug = workspace_slug(&workspace_id)?;
    if slug != workspace_id {
        return Err("The workspace identifier is invalid.".into());
    }
    let workspaces = read_workspaces(app)?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == slug)
        .ok_or_else(|| "That workspace is not in the Orvex workspace list.".to_owned())?;

    let home = app_lima_home(app)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start the Linux VM before opening a workspace terminal.".into());
    }
    let (guest_user, guest_home, passwordless_sudo) = guest_user(&home, instance_name)?;
    let (guest_uid, guest_gid) = if network_enabled {
        (None, None)
    } else {
        (
            Some(guest_identity_id(&home, instance_name, "-u")?),
            Some(guest_identity_id(&home, instance_name, "-g")?),
        )
    };
    if !network_enabled && !passwordless_sudo {
        return Err(
            "Offline terminals need passwordless sudo to create a Linux network namespace.".into(),
        );
    }
    if !network_enabled && (guest_uid == Some(0) || guest_gid == Some(0)) {
        return Err("Offline terminals need a non-root Lima guest user.".into());
    }
    let guest_path = guest_home.join("orvex/workspaces").join(&slug);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let path_check = run_limactl(
        &home,
        &["shell", instance_name, "test", "-d", &guest_path_text],
    )?;
    if !path_check.status.success() {
        return Err(format!(
            "Workspace folder was not found in Linux: {guest_path_text}"
        ));
    }

    let limactl = find_executable("limactl")
        .ok_or_else(|| "Lima is not installed. Install it before opening a terminal.".to_owned())?;
    let limactl_text = limactl.to_string_lossy();
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?;
    let sessions = app_data.join("terminal-sessions");
    std::fs::create_dir_all(&sessions)
        .map_err(|error| format!("Could not prepare terminal session files: {error}"))?;
    let mode = if network_enabled { "online" } else { "offline" };
    let script_path = sessions.join(format!("{slug}-{mode}.command"));

    let command = if network_enabled {
        format!(
            "exec {} shell --workdir {} {} bash -l\n",
            shell_quote(&limactl_text),
            shell_quote(&guest_path_text),
            shell_quote(instance_name),
        )
    } else {
        let guest_uid = guest_uid.expect("offline terminal has a guest UID");
        let guest_gid = guest_gid.expect("offline terminal has a guest GID");
        let guest_command = format!("cd {} && exec bash -l", shell_quote(&guest_path_text));
        format!(
            "exec {} shell --workdir {} {} sudo -n unshare --user --map-users=0:0:1 --map-users={guest_uid}:{guest_uid}:1 --map-groups=0:0:1 --map-groups={guest_gid}:{guest_gid}:1 --net --fork -- su -s /bin/bash -c {} {}\n",
            shell_quote(&limactl_text),
            shell_quote(&guest_path_text),
            shell_quote(instance_name),
            shell_quote(&guest_command),
            shell_quote(&guest_user),
        )
    };
    let script = format!(
        "#!/bin/zsh\nexport LIMA_HOME={}\n{}",
        shell_quote(&home.to_string_lossy()),
        command
    );
    std::fs::write(&script_path, script)
        .map_err(|error| format!("Could not prepare the terminal command: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not secure the terminal command: {error}"))?;
    }

    let output = Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&script_path)
        .output()
        .map_err(|error| format!("Could not open Terminal: {error}"))?;
    if !output.status.success() {
        return Err(output_error(&output, "open Terminal"));
    }
    Ok(())
}

fn cached_lima_image_bytes() -> Option<u64> {
    let home = std::env::var_os("HOME")?;
    let cache = PathBuf::from(home).join("Library/Caches/lima/download/by-url-sha256");
    let directories = std::fs::read_dir(cache).ok()?;
    let image_name = "ubuntu-26.04-server-cloudimg-arm64.img";
    let mut newest: Option<(std::time::SystemTime, u64)> = None;

    for directory in directories.flatten() {
        let entry_path = directory.path();
        let Ok(url) = std::fs::read_to_string(entry_path.join("url")) else {
            continue;
        };
        if !url.contains(image_name) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(entry_path) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name != "data" {
                continue;
            }
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
                newest = Some((modified, metadata.len()));
            }
        }
    }
    newest.map(|(_, bytes)| bytes)
}

fn make_vm_status(state: String, instance_name: &str) -> VmStatus {
    VmStatus {
        state,
        instance_name: instance_name.to_owned(),
        image_bytes: cached_lima_image_bytes(),
    }
}

fn start_instance_blocking(
    app: &AppHandle,
    instance_name: &str,
    workspace_id: Option<&str>,
) -> Result<VmStatus, String> {
    if !cfg!(target_os = "macos") {
        return Err("Lima virtual machines are currently supported on macOS only.".into());
    }
    let home = app_lima_home(app)?;
    let state = vm_state(&home, instance_name)?;
    let _ = app.emit(
        "vm-start-progress",
        VmProgress {
            phase: "checking",
            message: if state == "notCreated" {
                "Preparing a dedicated Linux VM for this workspace…".into()
            } else {
                "Resuming this workspace's Linux VM…".into()
            },
            downloaded_bytes: None,
            workspace_id: workspace_id.map(str::to_owned),
        },
    );
    if state == "notCreated" {
        let instance_arg = format!("--name={instance_name}");
        run_limactl_with_progress(
            app,
            &home,
            &[
                "start",
                &instance_arg,
                "--vm-type=vz",
                "--network=vzNAT",
                "--mount-none",
                "--containerd=none",
                "--cpus=2",
                "--memory=2",
                "--disk=8",
                "--progress",
                "--yes",
            ],
            workspace_id,
        )?;
    } else if state != "running" {
        run_limactl_with_progress(
            app,
            &home,
            &["start", instance_name, "--progress", "--yes"],
            workspace_id,
        )?;
    }
    let _ = app.emit(
        "vm-start-progress",
        VmProgress {
            phase: "ready",
            message: "This workspace's Linux VM is ready.".into(),
            downloaded_bytes: None,
            workspace_id: workspace_id.map(str::to_owned),
        },
    );
    Ok(make_vm_status(vm_state(&home, instance_name)?, instance_name))
}

fn start_poc_vm_blocking(app: &AppHandle) -> Result<VmStatus, String> {
    start_instance_blocking(app, POC_INSTANCE, None)
}

fn run_poc_check_blocking(app: &AppHandle) -> Result<String, String> {
    let home = app_lima_home(app)?;
    if vm_state(&home, POC_INSTANCE)? != "running" {
        return Err("Start the Orvex VM before running the check.".into());
    }
    let output = run_limactl(&home, &["shell", POC_INSTANCE, "uname", "-a"])?;
    if !output.status.success() {
        return Err(output_error(&output, "run the Linux check"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn stop_instance_blocking(
    app: &AppHandle,
    instance_name: &str,
    delete: bool,
) -> Result<VmStatus, String> {
    let home = app_lima_home(app)?;
    let state = vm_state(&home, instance_name)?;
    if state == "notCreated" {
        return Ok(make_vm_status(state, instance_name));
    }
    if delete {
        let output = run_limactl(&home, &["delete", "--force", instance_name, "--yes"])?;
        if !output.status.success() {
            return Err(output_error(&output, "delete the workspace VM"));
        }
        Ok(make_vm_status("notCreated".into(), instance_name))
    } else {
        if state == "running" {
            let output = run_limactl(&home, &["stop", instance_name, "--yes"])?;
            if !output.status.success() {
                return Err(output_error(&output, "stop the workspace VM"));
            }
        }
        Ok(make_vm_status(vm_state(&home, instance_name)?, instance_name))
    }
}

fn stop_poc_vm_blocking(app: &AppHandle, delete: bool) -> Result<VmStatus, String> {
    let directory = app.path().app_data_dir().map_err(|e|e.to_string())?;
    let _lock = if delete { Some(workspace_store::MutationLock::acquire(&directory)?) } else { None };
    let status = stop_instance_blocking(app, POC_INSTANCE, delete)?;
    if delete {
        let mut workspaces = read_workspaces(app)?;
        workspaces.retain(|workspace| workspace.vm_name.is_some());
        write_workspaces(app, &workspaces)?;
    }
    Ok(status)
}

fn workspace_record(app: &AppHandle, workspace_id: &str) -> Result<Workspace, String> {
    let slug = workspace_slug(workspace_id)?;
    if slug != workspace_id {
        return Err("The workspace identifier is invalid.".into());
    }
    read_workspaces(app)?
        .into_iter()
        .find(|workspace| workspace.id == slug)
        .ok_or_else(|| "That workspace is not in the Orvex workspace list.".to_owned())
}

fn agent_provider(provider_id: &str) -> Result<AgentProvider, String> {
    AGENT_PROVIDERS
        .iter()
        .find(|provider| provider.id == provider_id)
        .cloned()
        .ok_or_else(|| format!("Unsupported coding agent: {provider_id}"))
}

fn install_workspace_agent_blocking(
    home: &Path,
    instance_name: &str,
    workspace_id: &str,
    provider: &AgentProvider,
) -> AgentInstallResult {
    let guest_path = format!("$HOME/orvex/workspaces/{workspace_id}");
    let package = match provider.id {
        "codex" => "@openai/codex",
        "copilot" => "@github/copilot",
        "claude" => "@anthropic-ai/claude-code",
        "pi" => "@earendil-works/pi-coding-agent",
        "gemini" => "@google/gemini-cli",
        "cursor" => "",
        _ => unreachable!("provider validated by the catalog"),
    };
    // Node 22 LTS is the common supported baseline for the selected npm CLIs.
    // The official Node release index and checksum file are used to select and
    // verify the current 22.x binary for the guest architecture.
    let script = if provider.id == "cursor" {
        format!(
            r#"set -eu
mkdir -p "$HOME/.local/bin"
grep -Fqx 'export PATH="$HOME/.local/bin:$PATH"' "$HOME/.profile" 2>/dev/null || echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.profile"
curl -fsSL https://cursor.com/install | bash
"$HOME/.local/bin/agent" --version
"#
        )
    } else {
        format!(
            r#"set -eu
mkdir -p "$HOME/.local/bin" "$HOME/.local/lib"
grep -Fqx 'export PATH="$HOME/.local/bin:$PATH"' "$HOME/.profile" 2>/dev/null || echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.profile"
export PATH="$HOME/.local/bin:$PATH"
arch="$(uname -m)"
case "$arch" in
  aarch64|arm64) node_arch=arm64 ;;
  x86_64|amd64) node_arch=x64 ;;
  *) echo "Unsupported guest architecture: $arch" >&2; exit 1 ;;
esac
if ! command -v node >/dev/null 2>&1 || ! node -e 'const [major,minor]=process.versions.node.split(".").map(Number); process.exit(major > 22 || (major === 22 && minor >= 19) ? 0 : 1)' ; then
  command -v curl >/dev/null 2>&1 || {{ sudo apt-get update && sudo apt-get install -y curl ca-certificates xz-utils python3; }}
  version="$(curl -fsSL https://nodejs.org/dist/index.json | python3 -c 'import json,sys; print(next(x["version"] for x in json.load(sys.stdin) if x["version"].startswith("v22.")))')"
  archive="node-$version-linux-$node_arch.tar.xz"
  base="https://nodejs.org/dist/$version"
  curl -fsSL "$base/$archive" -o "/tmp/$archive"
  curl -fsSL "$base/SHASUMS256.txt" -o /tmp/node-shasums.txt
  (cd /tmp && grep "  $archive$" node-shasums.txt | sha256sum -c -)
  node_dir="$HOME/.local/lib/node-$version"
  mkdir -p "$node_dir"
  tar -xJf "/tmp/$archive" -C "$node_dir" --strip-components=1
  ln -sf "$node_dir/bin/node" "$HOME/.local/bin/node"
  ln -sf "$node_dir/bin/npm" "$HOME/.local/bin/npm"
  ln -sf "$node_dir/bin/npx" "$HOME/.local/bin/npx"
fi
export NPM_CONFIG_PREFIX="$HOME/.local"
npm install --global {package}
cd "{guest_path}"
"{executable}" --version
"#,
            executable = provider.executable,
            guest_path = guest_path,
            package = package,
        )
    };
    let result = run_limactl(
        home,
        &["shell", instance_name, "sh", "-lc", &script],
    );
    match result {
        Ok(output) if output.status.success() => {
            let version_text = if output.stdout.is_empty() { &output.stderr } else { &output.stdout };
            AgentInstallResult {
                provider_id: provider.id.to_owned(),
                provider_name: provider.name.to_owned(),
                installed: true,
                version: Some(String::from_utf8_lossy(version_text).trim().lines().last().unwrap_or("Installed").to_owned()),
                message: "Installed in this workspace VM.".into(),
                login_hint: provider.login_hint.to_owned(),
            }
        }
        Ok(output) => AgentInstallResult {
            provider_id: provider.id.to_owned(),
            provider_name: provider.name.to_owned(),
            installed: false,
            version: None,
            message: output_error(&output, &format!("install {}", provider.name)),
            login_hint: provider.login_hint.to_owned(),
        },
        Err(error) => AgentInstallResult {
            provider_id: provider.id.to_owned(),
            provider_name: provider.name.to_owned(),
            installed: false,
            version: None,
            message: error,
            login_hint: provider.login_hint.to_owned(),
        },
    }
}

fn validate_setup_commands(commands: &[String]) -> Result<(), String> {
    if commands.is_empty() || commands.len() > 10 {
        return Err("The setup manifest must contain between 1 and 10 commands.".into());
    }
    if commands.iter().any(|command| {
        let command = command.trim();
        command.is_empty() || command.len() > 1_000 || command.contains('\0')
    }) {
        return Err("Each setup command must be non-empty and no longer than 1,000 bytes.".into());
    }
    Ok(())
}

fn validate_compose_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 512
        || value.contains(['\\', '\n', '\r', '\0'])
        || path.is_absolute()
        || path.components().any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("Compose file must be a safe path relative to the repository root.".into());
    }
    Ok(())
}

fn detect_standard_compose_file(
    home: &Path,
    instance_name: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    let script = format!(
        "root=\"$HOME/orvex/workspaces/{workspace_id}\"; for file in compose.yaml compose.yml docker-compose.yaml docker-compose.yml; do if [ -f \"$root/$file\" ]; then printf '%s' \"$file\"; exit 0; fi; done; exit 44"
    );
    let output = run_limactl(home, &["shell", instance_name, "sh", "-lc", &script])?;
    if output.status.code() == Some(44) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(output_error(&output, "look for a Docker Compose file"));
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

fn discover_project_config(
    home: &Path,
    instance_name: &str,
    workspace_id: &str,
) -> Result<ProjectConfig, String> {
    let script = format!(
        "file=\"$HOME/orvex/workspaces/{workspace_id}/.orvex/workspace.json\"; if [ -f \"$file\" ]; then cat -- \"$file\"; else exit 44; fi"
    );
    let output = run_limactl(home, &["shell", instance_name, "sh", "-lc", &script])?;
    if output.status.code() == Some(44) {
        return Ok(ProjectConfig {
            setup_commands: Vec::new(),
            compose_file: detect_standard_compose_file(home, instance_name, workspace_id)?,
        });
    }
    if !output.status.success() {
        return Err(output_error(&output, "read the project setup manifest"));
    }
    let manifest: ProjectSetupManifest = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid .orvex/workspace.json: {error}"))?;
    if manifest.version != 1 {
        return Err(format!("Unsupported project setup manifest version {}.", manifest.version));
    }
    if !manifest.setup.commands.is_empty() {
        validate_setup_commands(&manifest.setup.commands)
            .map_err(|error| format!("Invalid .orvex/workspace.json: {error}"))?;
    }
    let configured_compose = manifest.services.and_then(|services| services.compose_file);
    let compose_file = if let Some(path) = configured_compose {
        validate_compose_path(&path)
            .map_err(|error| format!("Invalid .orvex/workspace.json: {error}"))?;
        let check = format!(
            "test -f \"$HOME/orvex/workspaces/{workspace_id}/\"{}",
            shell_quote(&path)
        );
        let check = run_limactl(home, &["shell", instance_name, "sh", "-lc", &check])?;
        if !check.status.success() {
            return Err(format!("The configured Compose file “{path}” was not found in the repository."));
        }
        Some(path)
    } else {
        detect_standard_compose_file(home, instance_name, workspace_id)?
    };
    Ok(ProjectConfig { setup_commands: manifest.setup.commands, compose_file })
}

fn run_workspace_setup_blocking(
    app: &AppHandle,
    workspace_id: &str,
    approved_commands: Vec<String>,
) -> Result<ProjectSetupResult, String> {
    validate_setup_commands(&approved_commands)?;
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let home = app_lima_home(app)?;
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start this workspace's Linux VM before running project setup.".into());
    }
    let current_config = discover_project_config(&home, instance_name, &workspace.id)?;
    if current_config.setup_commands != approved_commands {
        return Err("The project setup manifest changed after review. Recreate the workspace to review its current commands.".into());
    }
    let guest_path = format!("$HOME/orvex/workspaces/{}", workspace.id);
    let mut completed = 0;
    for command in &approved_commands {
        let script = format!("cd \"{guest_path}\" && sh -lc {}", shell_quote(command));
        let output = run_limactl(
            &home,
            &["shell", instance_name, "sh", "-lc", &script],
        )?;
        if !output.status.success() {
            let failed_command = command.clone();
            let message = output_error(&output, "run a project setup command");
            return Ok(ProjectSetupResult {
                commands: approved_commands,
                completed,
                failed_command: Some(failed_command),
                message,
            });
        }
        completed += 1;
    }
    Ok(ProjectSetupResult {
        commands: approved_commands,
        completed,
        failed_command: None,
        message: "Project setup completed.".into(),
    })
}

fn ensure_docker_blocking(home: &Path, instance_name: &str) -> Result<(), String> {
    const INSTALL_DOCKER: &str = r#"set -eu
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  sudo -n apt-get update
  sudo -n apt-get install -y ca-certificates curl
  sudo -n install -m 0755 -d /etc/apt/keyrings
  sudo -n curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
  sudo -n chmod a+r /etc/apt/keyrings/docker.asc
  arch="$(dpkg --print-architecture)"
  suite="$(. /etc/os-release && echo "${UBUNTU_CODENAME:-$VERSION_CODENAME}")"
  printf 'Types: deb\nURIs: https://download.docker.com/linux/ubuntu\nSuites: %s\nComponents: stable\nArchitectures: %s\nSigned-By: /etc/apt/keyrings/docker.asc\n' "$suite" "$arch" | sudo -n tee /etc/apt/sources.list.d/docker.sources >/dev/null
  sudo -n apt-get update
  sudo -n apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
fi
sudo -n systemctl enable --now docker
sudo -n usermod -aG docker "$(id -un)"
docker compose version
"#;
    let output = run_limactl(home, &["shell", instance_name, "sh", "-lc", INSTALL_DOCKER])?;
    if !output.status.success() {
        return Err(output_error(&output, "install Docker Engine and Compose in the workspace VM"));
    }
    Ok(())
}

fn discover_workspace_vm_ip(home: &Path, instance_name: &str) -> Option<String> {
    let output = run_limactl(
        home,
        &[
            "shell",
            instance_name,
            "sh",
            "-lc",
            "ip -o -4 addr show scope global",
        ],
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let words = line.split_whitespace().collect::<Vec<_>>();
            let index = words.iter().position(|word| *word == "inet")?;
            words.get(index + 1).map(|address| address.split('/').next().unwrap_or_default())
        })
        .filter(|address| !address.is_empty() && *address != "192.168.5.15" && !address.starts_with("127."))
        .last()
        .map(str::to_owned)
}

fn workspace_compose_file(
    app: &AppHandle,
    home: &Path,
    instance_name: &str,
    workspace: &Workspace,
) -> Result<Option<String>, String> {
    let compose_file = match workspace.compose_file.as_deref() {
        Some(path) => {
            validate_compose_path(path)?;
            Some(path.to_owned())
        }
        None => discover_project_config(home, instance_name, &workspace.id)?.compose_file,
    };
    if let Some(path) = &compose_file {
        let check = format!(
            "test -f \"$HOME/orvex/workspaces/{}/\"{}",
            workspace.id,
            shell_quote(path)
        );
        let output = run_limactl(home, &["shell", instance_name, "sh", "-lc", &check])?;
        if !output.status.success() {
            return Err(format!("Compose file “{path}” is missing from this workspace."));
        }
    }
    if workspace.compose_file != compose_file {
        let directory = app.path().app_data_dir().map_err(|e|e.to_string())?;
        let _lock = workspace_store::MutationLock::acquire(&directory)?;
        let mut workspaces = read_workspaces(app)?;
        if let Some(saved) = workspaces.iter_mut().find(|item| item.id == workspace.id) {
            saved.compose_file = compose_file.clone();
            write_workspaces(app, &workspaces)?;
        }
    }
    Ok(compose_file)
}

fn compose_command(
    action: &str,
    workspace_id: &str,
    compose_file: &str,
) -> Result<String, String> {
    validate_compose_path(compose_file)?;
    let project = format!("orvex-{workspace_id}");
    let command = match action {
        "start" => "up -d --remove-orphans",
        "stop" => "stop",
        "status" => "ps --all --format json",
        "logs" => "logs --tail 100 --no-color",
        _ => return Err("Unsupported workspace service action.".into()),
    };
    Ok(format!(
        "cd \"$HOME/orvex/workspaces/{workspace_id}\" && sudo -n docker compose --project-name {} --file {} {command}",
        shell_quote(&project),
        shell_quote(compose_file),
    ))
}

fn manage_workspace_services_blocking(
    app: &AppHandle,
    workspace_id: &str,
    action: &str,
    allow_network: bool,
) -> Result<WorkspaceServices, String> {
    if action == "start" && !allow_network {
        return Err("Approve network access before starting Compose services.".into());
    }
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace.vm_name.as_deref().ok_or_else(|| {
        "Services are available only in a dedicated workspace VM. This workspace uses the legacy shared VM.".to_owned()
    })?;
    let home = app_lima_home(app)?;
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start this workspace's Linux VM before managing services.".into());
    }
    let compose_file = workspace_compose_file(app, &home, instance_name, &workspace)?
        .ok_or_else(|| "No Compose file found. Add compose.yaml or declare services.composeFile in .orvex/workspace.json.".to_owned())?;
    let mut docker_installed = run_limactl(
        &home,
        &["shell", instance_name, "sh", "-lc", "command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1"],
    )
    .map(|result| result.status.success())
    .unwrap_or(false);
    if action == "start" {
        ensure_docker_blocking(&home, instance_name)?;
        docker_installed = true;
    } else if !docker_installed {
        return Ok(WorkspaceServices {
            workspace_id: workspace.id,
            compose_file: Some(compose_file),
            docker_installed: false,
            vm_ip: discover_workspace_vm_ip(&home, instance_name),
            output: "Docker Engine and Compose are not installed. Start services to install them.".into(),
            containers: Vec::new(),
            ports: Vec::new(),
        });
    }
    let command = compose_command(action, &workspace.id, &compose_file)?;
    let output = run_limactl(&home, &["shell", instance_name, "sh", "-lc", &command])?;
    if !output.status.success() {
        return Err(output_error(&output, &format!("{action} workspace services")));
    }
    let command_output = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let mut containers = Vec::new();
    let mut ports = Vec::new();
    if action == "status" || action == "start" || action == "stop" || action == "logs" {
        let status_command = compose_command("status", &workspace.id, &compose_file)?;
        let status = run_limactl(&home, &["shell", instance_name, "sh", "-lc", &status_command])?;
        if !status.status.success() {
            return Err(output_error(&status, "read workspace service status"));
        }
        let vm_ip = discover_workspace_vm_ip(&home, instance_name);
        for line in String::from_utf8_lossy(&status.stdout).lines().filter(|line| !line.trim().is_empty()) {
            let item: ComposePsLine = serde_json::from_str(line)
                .map_err(|error| format!("Could not parse Compose service status: {error}"))?;
            containers.push(ComposeContainer { service: item.service.clone(), state: item.state, health: item.health });
            for publisher in item.publishers.unwrap_or_default() {
                let Some(published_port) = publisher.published_port else { continue };
                let host_reachable = publisher.url == "0.0.0.0" || publisher.url == "::" || vm_ip.as_deref() == Some(&publisher.url);
                let likely_http = publisher.protocol == "tcp"
                    && matches!(publisher.target_port, 80 | 443 | 3000 | 4000 | 4173 | 5000 | 5173 | 8000 | 8080 | 8081 | 8888 | 9000);
                let url = (host_reachable && likely_http).then(|| {
                    let scheme = if publisher.target_port == 443 { "https" } else { "http" };
                    format!("{scheme}://{}:{published_port}", vm_ip.as_deref().unwrap_or("<workspace-vm-ip>"))
                });
                ports.push(WorkspacePort {
                    service: item.service.clone(),
                    target_port: publisher.target_port,
                    published_port,
                    protocol: publisher.protocol,
                    url,
                });
            }
        }
    }
    Ok(WorkspaceServices {
        workspace_id: workspace.id,
        compose_file: Some(compose_file),
        docker_installed,
        vm_ip: discover_workspace_vm_ip(&home, instance_name),
        output: if action == "status" { String::new() } else { command_output },
        containers,
        ports,
    })
}

fn check_workspace_agent_blocking(
    app: &AppHandle,
    workspace_id: &str,
    provider_id: &str,
) -> Result<AgentPreflight, String> {
    let workspace = workspace_record(app, workspace_id)?;
    let provider = agent_provider(provider_id)?;
    let home = app_lima_home(app)?;
    check_agent_in_workspace(&home, &workspace, &provider)
}

fn check_agent_in_workspace(
    home: &Path,
    workspace: &Workspace,
    provider: &AgentProvider,
) -> Result<AgentPreflight, String> {
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start this workspace's Linux VM before checking agent setup.".into());
    }

    let (_, guest_home, _) = guest_user(home, instance_name)?;
    let guest_path = guest_home.join("orvex/workspaces").join(&workspace.id);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let script = format!(
        "export PATH=\"$HOME/.local/bin:$PATH\"; if command -v {0} >/dev/null 2>&1; then {0} --version; else exit 127; fi",
        provider.executable
    );
    let output = run_limactl(
        home,
        &[
            "shell",
            "--workdir",
            &guest_path_text,
            instance_name,
            "sh",
            "-lc",
            &script,
        ],
    )?;
    if !output.status.success() && output.status.code() != Some(127) {
        return Err(output_error(&output, &format!("check {} in the guest", provider.name)));
    }
    let installed = output.status.success();
    let version = installed.then(|| {
        let version_text = if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        };
        String::from_utf8_lossy(version_text)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("Version not reported")
            .to_owned()
    });
    let message = if installed {
        "CLI found in this workspace VM. Sign-in is managed inside the guest.".to_owned()
    } else {
        format!(
            "{} was not found in this workspace VM. Install it inside the guest first.",
            provider.name
        )
    };

    Ok(AgentPreflight {
        workspace_id: workspace.id.clone(),
        provider_id: provider.id.to_owned(),
        provider_name: provider.name.to_owned(),
        installed,
        version,
        message,
        login_hint: provider.login_hint.to_owned(),
    })
}

fn workspace_vm_status_blocking(app: &AppHandle, workspace_id: &str) -> Result<VmStatus, String> {
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let home = app_lima_home(app)?;
    Ok(make_vm_status(vm_state(&home, instance_name)?, instance_name))
}

fn refresh_workspace_compose_file_blocking(
    app: &AppHandle,
    workspace_id: &str,
) -> Result<Workspace, String> {
    let workspace = workspace_record(app, workspace_id)?;
    let Some(instance_name) = workspace.vm_name.as_deref() else { return Ok(workspace) };
    let home = app_lima_home(app)?;
    if vm_state(&home, instance_name)? != "running" {
        return Ok(workspace);
    }
    let _ = workspace_compose_file(app, &home, instance_name, &workspace)?;
    workspace_record(app, workspace_id)
}

fn start_workspace_vm_blocking(app: &AppHandle, workspace_id: &str) -> Result<VmStatus, String> {
    let directory = app.path().app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?;
    let status = workspace_controller::WorkspaceController::new(directory)
        .set_running(workspace_id, true, true, |home, args| {
            run_limactl_with_progress(app, &home.to_path_buf(), args, Some(workspace_id))
        })?;
    let _ = app.emit("vm-start-progress", VmProgress {
        phase: "ready", message: "This workspace's Linux VM is ready.".into(),
        downloaded_bytes: None, workspace_id: Some(workspace_id.to_owned()),
    });
    Ok(status)
}

fn stop_workspace_vm_blocking(app: &AppHandle, workspace_id: &str) -> Result<VmStatus, String> {
    let directory = app.path().app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?;
    workspace_controller::WorkspaceController::new(directory)
        .set_running(workspace_id, false, false, |home, args| {
            let output = run_limactl(home, args)?;
            if !output.status.success() { return Err(output_error(&output, "stop the workspace VM")); }
            Ok(())
        })
}

fn run_workspace_check_blocking(app: &AppHandle, workspace_id: &str) -> Result<String, String> {
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let home = app_lima_home(app)?;
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start this workspace's Linux VM before running the check.".into());
    }
    let output = run_limactl(&home, &["shell", instance_name, "uname", "-a"])?;
    if !output.status.success() {
        return Err(output_error(&output, "run the workspace Linux check"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn delete_workspace_blocking(app: &AppHandle, workspace_id: &str) -> Result<(), String> {
    let directory = app.path().app_data_dir().map_err(|e|e.to_string())?;
    let controller = workspace_controller::WorkspaceController::new(directory);
    if controller.workspace(workspace_id)?.vm_name.is_some() {
        return controller.delete(workspace_id, true);
    }
    let _lock = workspace_store::MutationLock::acquire(&app.path().app_data_dir().map_err(|e|e.to_string())?)?;
    let mut workspaces = read_workspaces(app)?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .ok_or_else(|| "That workspace is not in the Orvex workspace list.".to_owned())?;

    if let Some(instance_name) = workspace.vm_name.as_deref() {
        let _ = stop_instance_blocking(app, instance_name, true)?;
        workspaces.retain(|item| item.id != workspace.id);
    } else {
        let legacy_count = workspaces.iter().filter(|item| item.vm_name.is_none()).count();
        if legacy_count != 1 {
            return Err(format!(
                "This workspace shares the legacy VM with {} other workspaces. Delete the shared legacy VM to remove them together.",
                legacy_count.saturating_sub(1),
            ));
        }
        let _ = stop_instance_blocking(app, POC_INSTANCE, true)?;
        workspaces.retain(|item| item.vm_name.is_some());
    }
    write_workspaces(app, &workspaces)
}

fn workspace_instance_name(slug: &str) -> String {
    // Lima appends a long SSH socket filename to this directory. A compact,
    // stable hash keeps the resulting Unix socket path below macOS's limit,
    // even when the workspace name itself is long.
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const HASH_SPACE: u64 = 2_821_109_907_456; // 36^8
    let hash = slug.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    }) % HASH_SPACE;
    let mut remaining = hash;
    let mut encoded = [b'0'; 8];
    for digit in encoded.iter_mut().rev() {
        *digit = ALPHABET[(remaining % 36) as usize];
        remaining /= 36;
    }
    format!("o{}", String::from_utf8_lossy(&encoded))
}

fn create_workspace_blocking(
    app: &AppHandle,
    name: String,
    repo_url: Option<String>,
    base_branch: Option<String>,
    allow_network: bool,
    agent_provider_ids: Vec<String>,
) -> Result<WorkspaceCreationResult, String> {
    let directory = app.path().app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?;
    create_workspace_at(&directory, name, repo_url, base_branch, allow_network, agent_provider_ids, None, false,
        |home, args, id| run_limactl_with_progress(app, &home.to_path_buf(), args, Some(id)))
}

fn create_workspace_at(
    data_dir: &Path,
    name: String,
    repo_url: Option<String>,
    base_branch: Option<String>,
    allow_network: bool,
    agent_provider_ids: Vec<String>,
    local_project: Option<PathBuf>,
    allow_unsupported_platform: bool,
    mut run: impl FnMut(&Path, &[&str], &str) -> Result<(), String>,
) -> Result<WorkspaceCreationResult, String> {
    if !cfg!(target_os = "macos") { return Err("Workspace VM creation is not available in this Windows UI beta.".into()); }
    let name = name.trim().to_owned();
    let slug = workspace_slug(&name)?;
    if local_project.is_some() && (repo_url.is_some() || base_branch.is_some()) {
        return Err("Use projectPath alone to import its current committed HEAD, or repoUrl with an optional branch.".into());
    }
    if !allow_network {
        return Err("Approve internet access to download Linux and create this workspace VM.".into());
    }
    let snapshot = local_project.as_deref().map(project_import::ProjectSnapshot::prepare).transpose()?;
    let preflight = match (&local_project, &snapshot) {
        (Some(path), Some(snapshot)) => Some(project_requirements::local(path, &snapshot.commit)?),
        _ => None,
    };
    if preflight.as_ref().is_some_and(|report| report.linux_compatibility == "unsupported") && !allow_unsupported_platform {
        return Err("Project requires an Apple platform toolchain (Xcode/Apple SDKs), which this Linux VM cannot run. Use project_inspect for details. Set allowUnsupportedPlatform=true only if you intentionally want a source-editing workspace.".into());
    }
    let repo_url = repo_url
        .filter(|url| !url.trim().is_empty())
        .map(|url| validate_public_https_repo_url(&url))
        .transpose()?;
    let base_branch = base_branch
        .filter(|branch| !branch.trim().is_empty())
        .map(|branch| branch.trim().to_owned());
    if base_branch.is_some() && repo_url.is_none() {
        return Err("Choose a repository before setting its base branch.".into());
    }
    let mut selected_providers = Vec::new();
    for provider_id in agent_provider_ids {
        let provider = agent_provider(&provider_id)?;
        if !selected_providers.iter().any(|selected: &AgentProvider| selected.id == provider.id) {
            selected_providers.push(provider);
        }
    }
    let _lock = workspace_store::MutationLock::acquire(data_dir)?;
    let mut workspaces = read_workspaces_at(&data_dir.join("workspaces.json"))?;
    if workspaces.iter().any(|workspace| workspace.id == slug) {
        return Err(format!("A workspace named “{name}” already exists."));
    }

    let repo_url = snapshot.as_ref().and_then(|s| s.origin.clone()).or(repo_url);
    let base_branch = snapshot.as_ref().map(|s| if s.branch.is_empty() { s.commit.clone() } else { s.branch.clone() }).or(base_branch);
    let instance_name = workspace_instance_name(&slug);
    let home = data_dir.join("lima");
    std::fs::create_dir_all(&home).map_err(|error| error.to_string())?;
    if vm_state(&home, &instance_name)? != "notCreated" {
        return Err("A VM with this workspace identity already exists. Inspect/delete it before recreating the workspace.".into());
    }
    let mut creation_guard = workspace_store::VmCreationGuard { home:home.clone(),instance:instance_name.clone(),committed:false };
    run(&home, &["start", &format!("--name={instance_name}"), "--vm-type=vz", "--network=vzNAT",
        "--mount-none", "--containerd=none", "--cpus=2", "--memory=2", "--disk=8", "--progress", "--yes"], &slug)?;

    let bundle_path = format!("/tmp/orvex-{slug}.bundle");
    if let Some(snapshot) = &snapshot {
        let destination = format!("{instance_name}:{bundle_path}");
        let output = run_limactl(&home, &["copy", snapshot.bundle.to_str().ok_or("Non-UTF-8 bundle path")?, &destination])?;
        if !output.status.success() { return Err(output_error(&output, "transfer project snapshot")); }
    }
    let clone_source = if snapshot.is_some() { bundle_path.as_str() } else { repo_url.as_deref().unwrap_or_default() };
    const CREATE_WORKSPACE_SCRIPT: &str = r#"set -eu
root="$HOME/orvex/workspaces"
target="$root/$2"
mkdir -p "$root"
if [ -e "$target" ] || [ -L "$target" ]; then
  printf 'Workspace path already exists. Choose another name.\n' >&2
  exit 17
fi
if [ -n "$1" ]; then
  staging="$(mktemp -d "$root/.$2.orvex-clone.XXXXXX")"
  rmdir "$staging"
  trap 'rm -rf -- "$staging"' EXIT
  GIT_TERMINAL_PROMPT=0 git clone --progress -- "$1" "$staging"
  base_branch="$3"
  base_ref=""
  if [ -n "$4" ]; then
    base_ref="$4"
    if [ -n "$5" ]; then git -C "$staging" remote set-url origin "$5"; else git -C "$staging" remote remove origin; fi
  elif [ -n "$base_branch" ]; then
    if ! git -C "$staging" check-ref-format --branch "$base_branch" >/dev/null 2>&1; then
      printf 'Invalid base branch name.\n' >&2
      exit 18
    fi
    base_ref="origin/$base_branch"
    if ! git -C "$staging" show-ref --verify --quiet "refs/remotes/$base_ref"; then
      printf 'Base branch was not found on origin: %s\n' "$base_branch" >&2
      exit 19
    fi
  elif git -C "$staging" rev-parse --verify HEAD >/dev/null 2>&1; then
    base_branch="$(git -C "$staging" branch --show-current)"
    base_ref="$base_branch"
  else
    base_branch=""
  fi
  session_branch="orvex/$2"
  if [ -n "$base_ref" ]; then
    git -C "$staging" switch --no-track --create "$session_branch" "$base_ref"
  else
    git -C "$staging" switch --no-track --orphan "$session_branch"
  fi
  mv -- "$staging" "$target"
  printf 'ORVEX_SESSION_BRANCH=%s\nORVEX_BASE_BRANCH=%s\n' "$session_branch" "$base_branch"
else
  mkdir "$target"
  printf '# Empty workspace\n\nCreated by Orvex.\n' > "$target/README.md"
  printf 'ORVEX_SESSION_BRANCH=No repository yet\nORVEX_BASE_BRANCH=\n'
fi
"#;
    let output = run_limactl(
        &home,
        &[
            "shell",
            &instance_name,
            "sh",
            "-c",
            CREATE_WORKSPACE_SCRIPT,
            "orvex-workspace",
            clone_source,
            &slug,
            base_branch.as_deref().unwrap_or_default(),
            snapshot.as_ref().map(|s| s.commit.as_str()).unwrap_or_default(),
            repo_url.as_deref().unwrap_or_default(),
        ],
    )?;
    if snapshot.is_some() { let _ = run_limactl(&home, &["shell", &instance_name, "rm", "-f", "--", &bundle_path]); }
    if !output.status.success() {
        return Err(output_error(&output, "create the Linux workspace"));
    }
    let creation_output = String::from_utf8_lossy(&output.stdout);
    let session_branch = creation_output
        .lines()
        .find_map(|line| line.strip_prefix("ORVEX_SESSION_BRANCH="))
        .ok_or_else(|| "The workspace was created, but its session branch was not reported.".to_owned())?;
    let base_branch = creation_output
        .lines()
        .find_map(|line| line.strip_prefix("ORVEX_BASE_BRANCH="))
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned);

    let requirements = if preflight.is_some() { preflight } else if repo_url.is_some() { Some(project_requirements::guest(&home, &instance_name, &slug)?) } else { None };
    if requirements.as_ref().is_some_and(|report| report.linux_compatibility == "unsupported") && !allow_unsupported_platform {
        return Err("The cloned project requires Xcode/Apple SDKs and cannot run in Linux. The newly allocated VM will be cleaned up. Use allowUnsupportedPlatform=true only for an intentional source-editing workspace.".into());
    }
    let (project_config, mut setup_error) = match discover_project_config(&home, &instance_name, &slug) {
        Ok(config) => (config, None),
        Err(error) => (ProjectConfig { setup_commands: Vec::new(), compose_file: None }, Some(error)),
    };
    // Creation authorizes project dependency installation inside this dedicated VM.
    // Keep a failed setup's checkout available for diagnosis, but never report it ready.
    let setup_result = if setup_error.is_none() && (snapshot.is_some() || repo_url.is_some()) {
        let needs_docker = project_config.compose_file.is_some() || requirements.as_ref().is_some_and(|report|
            report.evidence.iter().any(|item| item["kind"] == "container"));
        match provision_workspace_dependencies(&home, &instance_name, &slug, &project_config, needs_docker) {
            Ok(result) => {
                if result.failed_command.is_some() { setup_error = Some(result.message.clone()); }
                Some(result)
            }
            Err(error) => { setup_error = Some(error); None }
        }
    } else { None };
    let mut requirements = requirements;
    if let Some(report) = &mut requirements { report.automatic_setup = true; }
    let workspace = Workspace {
        id: slug.clone(),
        name,
        branch: session_branch.to_owned(),
        base_branch,
        status: if setup_error.is_some() { "Setup failed" } else { "Ready" }.into(),
        path: format!("~/orvex/workspaces/{slug}"),
        repo_url,
        vm_name: Some(instance_name),
        agent_providers: selected_providers.iter().map(|provider| provider.id.to_owned()).collect(),
        compose_file: project_config.compose_file.clone(),
    };
    workspaces.push(workspace.clone());
    write_workspaces_at(&data_dir.join("workspaces.json"), &workspaces)?;
    creation_guard.committed = true;
    let agent_installations = selected_providers
        .iter()
        .map(|provider| install_workspace_agent_blocking(&home, workspace.vm_name.as_deref().unwrap(), &slug, provider))
        .collect();
    let setup_commands = project_config.setup_commands;
    Ok(WorkspaceCreationResult { workspace, agent_installations, setup_commands, setup_error, setup_result, requirements })
}

fn provision_workspace_dependencies(
    home: &Path,
    instance: &str,
    id: &str,
    config: &ProjectConfig,
    needs_docker: bool,
) -> Result<ProjectSetupResult, String> {
    if needs_docker { ensure_docker_blocking(home, instance)?; }
    let mode = if config.setup_commands.is_empty() { "auto" } else { "custom" };
    let bootstrap = format!(
        "set -eu\ncd \"$HOME/orvex/workspaces/{id}\"\nexport PATH=\"/usr/local/bin:$HOME/.cargo/bin:$HOME/.dotnet:$PATH\"\nif ! command -v python3 >/dev/null; then sudo -n apt-get update; sudo -n apt-get install -y python3; fi\npython3 - {mode} <<'ORVEX_DEPENDENCIES'\n{}\nORVEX_DEPENDENCIES",
        include_str!("../scripts/setup_dependencies.py")
    );
    let mut commands = vec![bootstrap];
    commands.extend(config.setup_commands.iter().map(|command| format!(
        "cd \"$HOME/orvex/workspaces/{id}\" && export PATH=\"/usr/local/bin:$HOME/.cargo/bin:$HOME/.dotnet:$PATH\" && sh -lc {}", shell_quote(command)
    )));
    let labels = std::iter::once("Install detected project dependencies".to_owned())
        .chain(config.setup_commands.iter().cloned()).collect::<Vec<_>>();
    for (completed, command) in commands.iter().enumerate() {
        let output = run_limactl(home, &["shell", instance, "sh", "-lc", command])?;
        if !output.status.success() {
            return Ok(ProjectSetupResult { commands: labels.clone(), completed,
                failed_command: Some(labels[completed].clone()),
                message: output_error(&output, "install workspace dependencies") });
        }
    }
    Ok(ProjectSetupResult { completed: labels.len(), commands: labels,
        failed_command: None, message: "Project dependencies installed.".into() })
}

#[tauri::command]
async fn get_mcp_status(app: AppHandle) -> Result<mcp_connections::Status, String> {
    let directory=app.path().app_data_dir().map_err(|e|e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || mcp_connections::status(&directory)).await.map_err(|e|e.to_string())?
}

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        name: "Orvex",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
async fn list_workspaces(app: AppHandle) -> Result<Vec<Workspace>, String> {
    tauri::async_runtime::spawn_blocking(move || read_workspaces(&app))
        .await
        .map_err(|error| format!("Workspace list task failed: {error}"))?
}

#[tauri::command]
async fn create_workspace(
    app: AppHandle,
    name: String,
    repo_url: Option<String>,
    base_branch: Option<String>,
    allow_network: bool,
    agent_provider_ids: Vec<String>,
) -> Result<WorkspaceCreationResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_workspace_blocking(&app, name, repo_url, base_branch, allow_network, agent_provider_ids)
    })
    .await
    .map_err(|error| format!("Workspace creation task failed: {error}"))?
}

#[tauri::command]
async fn run_workspace_setup(
    app: AppHandle,
    workspace_id: String,
    approved_commands: Vec<String>,
) -> Result<ProjectSetupResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_workspace_setup_blocking(&app, &workspace_id, approved_commands)
    })
    .await
    .map_err(|error| format!("Project setup task failed: {error}"))?
}

#[tauri::command]
async fn manage_workspace_services(
    app: AppHandle,
    workspace_id: String,
    action: String,
    allow_network: bool,
) -> Result<WorkspaceServices, String> {
    tauri::async_runtime::spawn_blocking(move || {
        manage_workspace_services_blocking(&app, &workspace_id, &action, allow_network)
    })
    .await
    .map_err(|error| format!("Workspace services task failed: {error}"))?
}

#[tauri::command]
async fn open_workspace_terminal(
    app: AppHandle,
    workspace_id: String,
    network_enabled: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_workspace_terminal_blocking(&app, workspace_id, network_enabled)
    })
    .await
    .map_err(|error| format!("Terminal task failed: {error}"))?
}

#[tauri::command]
fn list_agent_providers() -> Vec<AgentProvider> {
    AGENT_PROVIDERS.to_vec()
}

#[tauri::command]
async fn check_workspace_agent(
    app: AppHandle,
    workspace_id: String,
    provider_id: String,
) -> Result<AgentPreflight, String> {
    tauri::async_runtime::spawn_blocking(move || {
        check_workspace_agent_blocking(&app, &workspace_id, &provider_id)
    })
    .await
    .map_err(|error| format!("Agent preflight task failed: {error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliWorkspace {
    id: String,
    name: String,
    branch: String,
    base_branch: Option<String>,
    path: String,
    vm_name: Option<String>,
    vm_state: String,
    agent_providers: Vec<String>,
    compose_file: Option<String>,
}

fn cli_app_data_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| "Could not find the current user's home directory.".to_owned())?;
        return Ok(PathBuf::from(home).join("Library/Application Support/com.orvex.desktop"));
    }
    #[cfg(target_os = "windows")]
    {
        let app_data = std::env::var_os("APPDATA")
            .ok_or_else(|| "Could not find the Windows roaming app data directory.".to_owned())?;
        return Ok(PathBuf::from(app_data).join("com.orvex.desktop"));
    }
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .ok_or_else(|| "Could not find the current user's data directory.".to_owned())?;
        if !base.is_absolute() {
            return Err("XDG_DATA_HOME must be an absolute path.".into());
        }
        return Ok(base.join("com.orvex.desktop"));
    }
    #[allow(unreachable_code)]
    Err("The Orvex CLI is not supported on this operating system yet.".into())
}

fn cli_workspace_context(workspace_id: &str) -> Result<(Workspace, PathBuf, String), String> {
    let data_dir = cli_app_data_dir()?;
    let workspace = workspace_controller::WorkspaceController::new(data_dir.clone())
        .workspace(workspace_id)?;
    let instance_name = workspace
        .vm_name
        .clone()
        .ok_or_else(|| "This workspace uses the legacy shared VM. CLI execution requires a dedicated workspace VM.".to_owned())?;
    Ok((workspace, data_dir.join("lima"), instance_name))
}

fn cli_list_workspaces(json: bool) -> Result<i32, String> {
    let data_dir = cli_app_data_dir()?;
    let workspaces = workspace_controller::WorkspaceController::new(data_dir.clone()).list()?;
    let lima_home = data_dir.join("lima");
    let mut items = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
        let vm_state = vm_state(&lima_home, instance_name).unwrap_or_else(|error| format!("unavailable: {error}"));
        items.push(CliWorkspace {
            id: workspace.id,
            name: workspace.name,
            branch: workspace.branch,
            base_branch: workspace.base_branch,
            path: workspace.path,
            vm_name: workspace.vm_name,
            vm_state,
            agent_providers: workspace.agent_providers,
            compose_file: workspace.compose_file,
        });
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&items).map_err(|error| error.to_string())?);
    } else if items.is_empty() {
        println!("No Orvex workspaces yet.");
    } else {
        println!("ID\tNAME\tVM\tBRANCH\tPATH");
        for item in items {
            println!("{}\t{}\t{}\t{}\t{}", item.id, item.name, item.vm_state, item.branch, item.path);
        }
    }
    Ok(0)
}

fn cli_workspace_status(workspace_id: &str, json: bool) -> Result<i32, String> {
    let data_dir = cli_app_data_dir()?;
    let workspace = workspace_controller::WorkspaceController::new(data_dir.clone())
        .workspace(workspace_id)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let state = vm_state(&data_dir.join("lima"), instance_name)?;
    let item = CliWorkspace {
        id: workspace.id,
        name: workspace.name,
        branch: workspace.branch,
        base_branch: workspace.base_branch,
        path: workspace.path,
        vm_name: workspace.vm_name,
        vm_state: state,
        agent_providers: workspace.agent_providers,
        compose_file: workspace.compose_file,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&item).map_err(|error| error.to_string())?);
    } else {
        println!("Workspace: {} ({})\nVM: {}\nBranch: {}\nPath: {}\nAgents: {}\nCompose: {}",
            item.name,
            item.id,
            item.vm_state,
            item.branch,
            item.path,
            if item.agent_providers.is_empty() { "none".to_owned() } else { item.agent_providers.join(", ") },
            item.compose_file.as_deref().unwrap_or("none"),
        );
    }
    Ok(0)
}

fn cli_workspace_vm_action(workspace_id: &str, start: bool, network: bool) -> Result<i32, String> {
    let status = workspace_controller::WorkspaceController::new(cli_app_data_dir()?)
        .set_running(workspace_id, start, network, |home, args| {
            let output = run_limactl(home, args)?;
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            if !output.status.success() { return Err(output_error(&output, "change workspace VM state")); }
            Ok(())
        })?;
    println!("Workspace VM `{}` is {}.", status.instance_name, status.state);
    Ok(0)
}

fn cli_workspace_exec(workspace_id: &str, network: bool, command: &[String]) -> Result<i32, String> {
    let controller = workspace_controller::WorkspaceController::new(cli_app_data_dir()?);
    let status = controller.command(workspace_id, network, command)?.status()
        .map_err(|error| format!("Could not start the Lima command: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

fn workspace_command(controller: &workspace_controller::WorkspaceController, workspace_id: &str,
    network: bool, command: &[String]) -> Result<Command, String> {
    if command.is_empty() {
        return Err("Provide a command after `--`, for example `orvex workspace exec <id> -- codex --version`.".into());
    }
    let workspace = controller.workspace(workspace_id)?;
    let instance_name = workspace.vm_name.clone().ok_or("A dedicated workspace VM is required.")?;
    let lima_home = controller.lima_home();
    if vm_state(&lima_home, &instance_name)? != "running" {
        return Err("Start the workspace VM in Orvex before running a command.".into());
    }
    let (guest_user, guest_home, passwordless_sudo) = guest_user(&lima_home, &instance_name)?;
    let guest_path = guest_home.join("orvex/workspaces").join(&workspace.id);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let path_check = run_limactl(&lima_home, &["shell", &instance_name, "test", "-d", &guest_path_text])?;
    if !path_check.status.success() {
        return Err(format!("Workspace folder was not found in Linux: {guest_path_text}"));
    }
    let limactl = find_executable("limactl")
        .ok_or_else(|| "Lima is not installed. Install it before using Orvex CLI commands.".to_owned())?;
    let mut invocation = Command::new(limactl);
    invocation.env("LIMA_HOME", &lima_home).args(["shell", "--workdir", &guest_path_text, &instance_name]);
    if network {
        invocation.args(command);
    } else {
        if !passwordless_sudo {
            return Err("Offline command execution needs passwordless sudo to create a guest network namespace.".into());
        }
        let uid = guest_identity_id(&lima_home, &instance_name, "-u")?;
        let gid = guest_identity_id(&lima_home, &instance_name, "-g")?;
        if uid == 0 || gid == 0 {
            return Err("Offline command execution needs a non-root Lima guest user.".into());
        }
        let command_text = command.iter().map(|part| shell_quote(part)).collect::<Vec<_>>().join(" ");
        let guest_command = format!("cd {} && exec {command_text}", shell_quote(&guest_path_text));
        invocation.args([
            "sudo", "-n", "unshare", "--user", "--map-users=0:0:1",
            &format!("--map-users={uid}:{uid}:1"), "--map-groups=0:0:1",
            &format!("--map-groups={gid}:{gid}:1"), "--net", "--fork", "--",
            "su", "-s", "/bin/bash", "-c", &guest_command, &guest_user,
        ]);
    }
    Ok(invocation)
}

fn cli_agent_check(workspace_id: &str, provider_id: &str, json: bool) -> Result<i32, String> {
    let (workspace, lima_home, _) = cli_workspace_context(workspace_id)?;
    let provider = agent_provider(provider_id)?;
    let result = check_agent_in_workspace(&lima_home, &workspace, &provider)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?);
    } else {
        println!("{}: {}", result.provider_name, result.message);
        if let Some(version) = result.version {
            println!("Version: {version}");
        }
        println!("{}", result.login_hint);
        println!("Authentication is not checked by Orvex.");
    }
    Ok(if result.installed { 0 } else { 1 })
}

fn print_cli_help() {
    println!("Orvex CLI — workspace access for scripts and the user's main agent\n\nUSAGE:\n  orvex tool <tool-name> '<JSON arguments>'\n  orvex mcp-config [--copilot]\n  orvex mcp\n  orvex workspace create <name> (--project <absolute-path> | --repo <https-url> [--branch <branch>]) --network\n  orvex workspace list [--json]\n  orvex workspace status <workspace-id> [--json]\n  orvex workspace start <workspace-id> --network\n  orvex workspace stop <workspace-id>\n  orvex workspace exec <workspace-id> [--network] -- <command> [args...]\n  orvex agent check <workspace-id> <provider-id> [--json]\n\nEXECUTION POLICY:\n  VM start requires the explicit --network grant. Commands run in an already-running VM and are offline by default.\n  Add --network before -- only when you approve network access for that command.\n  Orvex does not start coding agents from the desktop UI; exec runs only the command you provide.\n\nPROVIDER IDS: codex, claude, copilot, pi, cursor, gemini");
}

pub fn cli_main() -> i32 {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("tool") {
        let result = (|| {
            if args.len() != 3 { return Err("Usage: orvex tool <tool-name> '<JSON arguments>'".to_owned()); }
            let values = serde_json::from_str(&args[2]).map_err(|e| format!("Invalid JSON: {e}"))?;
            mcp::call(&args[1], &values)
        })();
        return match result { Ok(value) => { println!("{value}"); 0 }, Err(error) => { eprintln!("{error}"); 1 } };
    }
    if args.first().map(String::as_str) == Some("mcp-config") {
        let executable = match std::env::current_exe() { Ok(p) => p, Err(e) => { eprintln!("{e}"); return 1 } };
        let home = std::env::var("HOME").unwrap_or_default();
        let config = if args.get(1).map(String::as_str) == Some("--copilot") {
            serde_json::json!({"mcpServers":{"orvex":{"type":"stdio","command":executable,"args":["mcp"],"env":{"HOME":home},"tools":["*"],"timeout":600000}}})
        } else {
            serde_json::json!({"servers":{"orvex":{"type":"stdio","command":executable,"args":["mcp"],"env":{"HOME":home}}}})
        };
        println!("{config}");
        return 0;
    }
    if args.first().map(String::as_str) == Some("mcp") {
        return match mcp::serve() { Ok(()) => 0, Err(error) => { eprintln!("{error}"); 1 } };
    }
    if args.first().map(String::as_str) == Some("workspace") && args.get(1).map(String::as_str) == Some("create") {
        return match mcp::cli_create(&args[2..]) { Ok(value) => { println!("{value}"); 0 }, Err(error) => { eprintln!("{error}"); 1 } };
    }
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_cli_help();
        return 0;
    }
    if args.len() == 1 && args[0] == "--version" {
        println!("orvex {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if !cfg!(target_os = "macos") {
        eprintln!("orvex: workspace execution currently requires the macOS Lima runtime.");
        return 1;
    }
    let result = match args.first().map(String::as_str) {
        Some("workspace") if args.get(1).map(String::as_str) == Some("list") => {
            let json = args.iter().any(|arg| arg == "--json");
            if args.iter().any(|arg| arg != "workspace" && arg != "list" && arg != "--json") {
                Err("Unexpected argument for `workspace list`.".into())
            } else { cli_list_workspaces(json) }
        }
        Some("workspace") if args.get(1).map(String::as_str) == Some("status") => {
            match (args.get(2), args.get(3)) {
                (Some(id), None) => cli_workspace_status(id, false),
                (Some(id), Some(json)) if json == "--json" && args.len() == 4 => cli_workspace_status(id, true),
                _ => Err("Usage: orvex workspace status <workspace-id> [--json]".into()),
            }
        }
        Some("workspace") if args.get(1).map(String::as_str) == Some("start") => {
            match (args.get(2), args.get(3), args.len()) {
                (Some(id), Some(network), 4) if network == "--network" => cli_workspace_vm_action(id, true, true),
                (Some(id), None, 3) => cli_workspace_vm_action(id, true, false),
                _ => Err("Usage: orvex workspace start <workspace-id> --network".into()),
            }
        }
        Some("workspace") if args.get(1).map(String::as_str) == Some("stop") => {
            match (args.get(2), args.len()) {
                (Some(id), 3) => cli_workspace_vm_action(id, false, false),
                _ => Err("Usage: orvex workspace stop <workspace-id>".into()),
            }
        }
        Some("workspace") if args.get(1).map(String::as_str) == Some("exec") => {
            let Some(id) = args.get(2) else {
                eprintln!("Usage: orvex workspace exec <workspace-id> [--network] -- <command> [args...]");
                return 2;
            };
            let mut network = false;
            let mut index = 3;
            while index < args.len() && args[index] != "--" {
                if args[index] == "--network" { network = true; } else {
                    eprintln!("Unknown option `{}` before `--`.", args[index]);
                    return 2;
                }
                index += 1;
            }
            if args.get(index).map(String::as_str) != Some("--") {
                Err("Separate Orvex options from the guest command with `--`.".into())
            } else {
                cli_workspace_exec(id, network, &args[index + 1..])
            }
        }
        Some("agent") if args.get(1).map(String::as_str) == Some("check") => {
            match (args.get(2), args.get(3), args.get(4), args.len()) {
                (Some(id), Some(provider), None, 4) => cli_agent_check(id, provider, false),
                (Some(id), Some(provider), Some(json), 5) if json == "--json" => cli_agent_check(id, provider, true),
                _ => Err("Usage: orvex agent check <workspace-id> <provider-id> [--json]".into()),
            }
        }
        _ => Err("Unknown command. Run `orvex --help` for usage.".into()),
    };
    match result {
        Ok(code) => code,
        Err(error) => { eprintln!("orvex: {error}"); 1 }
    }
}

#[tauri::command]
fn get_lima_status() -> LimaStatus {
    inspect_lima()
}

#[tauri::command]
async fn install_lima() -> Result<LimaStatus, String> {
    tauri::async_runtime::spawn_blocking(install_lima_blocking)
        .await
        .map_err(|error| format!("Installer task failed: {error}"))?
}

#[tauri::command]
async fn get_vm_status(app: AppHandle) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if find_executable("limactl").is_none() {
            return Ok(make_vm_status("notCreated".into(), POC_INSTANCE));
        }
        let home = app_lima_home(&app)?;
        Ok(make_vm_status(vm_state(&home, POC_INSTANCE)?, POC_INSTANCE))
    })
    .await
    .map_err(|error| format!("VM status task failed: {error}"))?
}

#[tauri::command]
async fn get_workspace_vm_status(
    app: AppHandle,
    workspace_id: String,
) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        workspace_vm_status_blocking(&app, &workspace_id)
    })
    .await
    .map_err(|error| format!("Workspace VM status task failed: {error}"))?
}

#[tauri::command]
async fn refresh_workspace_compose_file(
    app: AppHandle,
    workspace_id: String,
) -> Result<Workspace, String> {
    tauri::async_runtime::spawn_blocking(move || {
        refresh_workspace_compose_file_blocking(&app, &workspace_id)
    })
    .await
    .map_err(|error| format!("Compose discovery task failed: {error}"))?
}

#[tauri::command]
async fn start_workspace_vm(
    app: AppHandle,
    workspace_id: String,
) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        start_workspace_vm_blocking(&app, &workspace_id)
    })
    .await
    .map_err(|error| format!("Workspace VM start task failed: {error}"))?
}

#[tauri::command]
async fn stop_workspace_vm(
    app: AppHandle,
    workspace_id: String,
) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        stop_workspace_vm_blocking(&app, &workspace_id)
    })
    .await
    .map_err(|error| format!("Workspace VM stop task failed: {error}"))?
}

#[tauri::command]
async fn run_workspace_check(
    app: AppHandle,
    workspace_id: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_workspace_check_blocking(&app, &workspace_id)
    })
    .await
    .map_err(|error| format!("Workspace Linux check task failed: {error}"))?
}

#[tauri::command]
async fn delete_workspace(app: AppHandle, workspace_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_workspace_blocking(&app, &workspace_id))
        .await
        .map_err(|error| format!("Workspace deletion task failed: {error}"))?
}

#[tauri::command]
async fn start_poc_vm(app: AppHandle) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || start_poc_vm_blocking(&app))
        .await
        .map_err(|error| format!("VM start task failed: {error}"))?
}

#[tauri::command]
async fn run_poc_check(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || run_poc_check_blocking(&app))
        .await
        .map_err(|error| format!("Linux check task failed: {error}"))?
}

#[tauri::command]
async fn stop_poc_vm(app: AppHandle) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || stop_poc_vm_blocking(&app, false))
        .await
        .map_err(|error| format!("VM stop task failed: {error}"))?
}

#[tauri::command]
async fn delete_poc_vm(app: AppHandle) -> Result<VmStatus, String> {
    tauri::async_runtime::spawn_blocking(move || stop_poc_vm_blocking(&app, true))
        .await
        .map_err(|error| format!("VM delete task failed: {error}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            get_mcp_status,
            list_workspaces,
            create_workspace,
            run_workspace_setup,
            manage_workspace_services,
            open_workspace_terminal,
            list_agent_providers,
            check_workspace_agent,
            get_lima_status,
            install_lima,
            get_vm_status,
            get_workspace_vm_status,
            refresh_workspace_compose_file,
            start_workspace_vm,
            stop_workspace_vm,
            run_workspace_check,
            delete_workspace,
            start_poc_vm,
            run_poc_check,
            stop_poc_vm,
            delete_poc_vm
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orvex");
}
