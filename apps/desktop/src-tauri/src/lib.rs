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
    let candidates = if name == "limactl" {
        vec![
            PathBuf::from("/opt/homebrew/bin/limactl"),
            PathBuf::from("/usr/local/bin/limactl"),
            PathBuf::from("/opt/local/bin/limactl"),
        ]
    } else {
        vec![
            PathBuf::from("/opt/homebrew/bin/brew"),
            PathBuf::from("/usr/local/bin/brew"),
        ]
    };
    candidates
        .into_iter()
        .chain(std::env::var_os("PATH").into_iter().flat_map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
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
    let path = workspace_store_path(app)?;
    match std::fs::read(&path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|error| format!("Could not read workspace list: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read workspace list: {error}")),
    }
}

fn write_workspaces(app: &AppHandle, workspaces: &[Workspace]) -> Result<(), String> {
    let path = workspace_store_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Could not locate Orvex app data directory.".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not prepare Orvex app data: {error}"))?;
    let data = serde_json::to_vec_pretty(workspaces)
        .map_err(|error| format!("Could not encode workspace list: {error}"))?;
    std::fs::write(path, data).map_err(|error| format!("Could not save workspace list: {error}"))
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
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let home = app_lima_home(app)?;
    if vm_state(&home, instance_name)? != "running" {
        return Err("Start this workspace's Linux VM before checking agent setup.".into());
    }

    let (_, guest_home, _) = guest_user(&home, instance_name)?;
    let guest_path = guest_home.join("orvex/workspaces").join(&workspace.id);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let script = format!(
        "export PATH=\"$HOME/.local/bin:$PATH\"; if command -v {0} >/dev/null 2>&1; then {0} --version; else exit 127; fi",
        provider.executable
    );
    let output = run_limactl(
        &home,
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
        workspace_id: workspace.id,
        provider_id: provider.id.to_owned(),
        provider_name: provider.name.to_owned(),
        installed,
        version,
        message,
        login_hint: provider.login_hint.to_owned(),
    })
}

fn open_workspace_agent_blocking(
    app: &AppHandle,
    workspace_id: &str,
    provider_id: &str,
    allow_network: bool,
) -> Result<(), String> {
    if !allow_network {
        return Err("Approve network access before starting a cloud coding agent.".into());
    }
    let preflight = check_workspace_agent_blocking(app, workspace_id, provider_id)?;
    if !preflight.installed {
        return Err(preflight.message);
    }
    if !cfg!(target_os = "macos") {
        return Err("Opening an agent terminal is currently supported on macOS only.".into());
    }

    let workspace = workspace_record(app, workspace_id)?;
    let provider = agent_provider(provider_id)?;
    let instance_name = workspace.vm_name.as_deref().unwrap_or(POC_INSTANCE);
    let home = app_lima_home(app)?;
    let (_, guest_home, _) = guest_user(&home, instance_name)?;
    let guest_path = guest_home.join("orvex/workspaces").join(&workspace.id);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let limactl = find_executable("limactl")
        .ok_or_else(|| "Lima is not installed. Install it before starting an agent.".to_owned())?;
    let sessions = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not locate Orvex app data: {error}"))?
        .join("terminal-sessions");
    std::fs::create_dir_all(&sessions)
        .map_err(|error| format!("Could not prepare terminal session files: {error}"))?;
    let script_path = sessions.join(format!("{}-agent-{}.command", workspace.id, provider.id));
    let guest_command = format!(
        "export PATH=\"$HOME/.local/bin:$PATH\"; exec {}",
        shell_quote(provider.executable),
    );
    let script = format!(
        "#!/bin/zsh\nexport LIMA_HOME={}\nexec {} shell --workdir {} {} sh -lc {}\n",
        shell_quote(&home.to_string_lossy()),
        shell_quote(&limactl.to_string_lossy()),
        shell_quote(&guest_path_text),
        shell_quote(instance_name),
        shell_quote(&guest_command),
    );
    std::fs::write(&script_path, script)
        .map_err(|error| format!("Could not prepare the agent command: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not secure the agent command: {error}"))?;
    }
    let output = Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&script_path)
        .output()
        .map_err(|error| format!("Could not open Terminal: {error}"))?;
    if !output.status.success() {
        return Err(output_error(&output, "open the agent terminal"));
    }
    Ok(())
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
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace
        .vm_name
        .as_deref()
        .ok_or_else(|| "This older workspace uses the shared VM. Start it from the shared VM panel.".to_owned())?;
    start_instance_blocking(app, instance_name, Some(workspace_id))
}

fn stop_workspace_vm_blocking(app: &AppHandle, workspace_id: &str) -> Result<VmStatus, String> {
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace
        .vm_name
        .as_deref()
        .ok_or_else(|| "This older workspace uses the shared VM. Stop it from the shared VM panel.".to_owned())?;
    stop_instance_blocking(app, instance_name, false)
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
    let workspace = workspace_record(app, workspace_id)?;
    let instance_name = workspace
        .vm_name
        .as_deref()
        .ok_or_else(|| "This older workspace shares a VM with other workspaces and cannot be deleted individually yet.".to_owned())?;
    let _ = stop_instance_blocking(app, instance_name, true)?;
    let mut workspaces = read_workspaces(app)?;
    workspaces.retain(|item| item.id != workspace.id);
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
    allow_network: bool,
    agent_provider_ids: Vec<String>,
) -> Result<WorkspaceCreationResult, String> {
    let name = name.trim().to_owned();
    let slug = workspace_slug(&name)?;
    let repo_url = repo_url
        .filter(|url| !url.trim().is_empty())
        .map(|url| validate_public_https_repo_url(&url))
        .transpose()?;
    if !allow_network {
        return Err("Approve internet access to download Linux and create this workspace VM.".into());
    }
    if agent_provider_ids.is_empty() {
        return Err("Choose at least one coding CLI to install in this workspace.".into());
    }
    let mut selected_providers = Vec::new();
    for provider_id in agent_provider_ids {
        let provider = agent_provider(&provider_id)?;
        if !selected_providers.iter().any(|selected: &AgentProvider| selected.id == provider.id) {
            selected_providers.push(provider);
        }
    }
    let mut workspaces = read_workspaces(app)?;
    if workspaces.iter().any(|workspace| workspace.id == slug) {
        return Err(format!("A workspace named “{name}” already exists."));
    }

    let instance_name = workspace_instance_name(&slug);
    let _ = start_instance_blocking(app, &instance_name, Some(&slug))?;
    let home = app_lima_home(app)?;

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
  mv -- "$staging" "$target"
else
  mkdir "$target"
  printf '# Empty workspace\n\nCreated by Orvex.\n' > "$target/README.md"
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
            repo_url.as_deref().unwrap_or_default(),
            &slug,
        ],
    )?;
    if !output.status.success() {
        return Err(output_error(&output, "create the Linux workspace"));
    }

    let (project_config, setup_error) = match discover_project_config(&home, &instance_name, &slug) {
        Ok(config) => (config, None),
        Err(error) => (ProjectConfig { setup_commands: Vec::new(), compose_file: None }, Some(error)),
    };
    let workspace = Workspace {
        id: slug.clone(),
        name,
        branch: if repo_url.is_some() {
            "Repository cloned".into()
        } else {
            "No repository yet".into()
        },
        status: "Ready".into(),
        path: format!("~/orvex/workspaces/{slug}"),
        repo_url,
        vm_name: Some(instance_name),
        agent_providers: selected_providers.iter().map(|provider| provider.id.to_owned()).collect(),
        compose_file: project_config.compose_file.clone(),
    };
    workspaces.push(workspace.clone());
    write_workspaces(app, &workspaces)?;
    let agent_installations = selected_providers
        .iter()
        .map(|provider| install_workspace_agent_blocking(&home, workspace.vm_name.as_deref().unwrap(), &slug, provider))
        .collect();
    let setup_commands = project_config.setup_commands;
    Ok(WorkspaceCreationResult { workspace, agent_installations, setup_commands, setup_error })
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
    allow_network: bool,
    agent_provider_ids: Vec<String>,
) -> Result<WorkspaceCreationResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_workspace_blocking(&app, name, repo_url, allow_network, agent_provider_ids)
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

#[tauri::command]
async fn open_workspace_agent(
    app: AppHandle,
    workspace_id: String,
    provider_id: String,
    allow_network: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_workspace_agent_blocking(&app, &workspace_id, &provider_id, allow_network)
    })
    .await
    .map_err(|error| format!("Agent launch task failed: {error}"))?
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
            list_workspaces,
            create_workspace,
            run_workspace_setup,
            manage_workspace_services,
            open_workspace_terminal,
            list_agent_providers,
            check_workspace_agent,
            open_workspace_agent,
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
