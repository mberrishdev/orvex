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
    instance_name: &'static str,
    image_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VmProgress {
    phase: &'static str,
    message: String,
    downloaded_bytes: Option<u64>,
}

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

fn run_limactl(home: &PathBuf, args: &[&str]) -> Result<std::process::Output, String> {
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

fn map_lima_progress(line: &str) -> Option<VmProgress> {
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

fn monitor_lima_download(app: AppHandle, pid: u32, stop: Arc<AtomicBool>) {
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

fn run_limactl_with_progress(app: &AppHandle, home: &PathBuf, args: &[&str]) -> Result<(), String> {
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
        thread::spawn(move || monitor_lima_download(app, pid, stop))
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
        if let Some(progress) = map_lima_progress(&line) {
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

fn vm_state(home: &PathBuf) -> Result<String, String> {
    let output = run_limactl(home, &["list", "--json"])?;
    if !output.status.success() {
        return Err(output_error(&output, "read VM status"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let instance: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("Could not parse Lima status: {error}"))?;
        if instance.get("name").and_then(|name| name.as_str()) == Some(POC_INSTANCE) {
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

fn guest_user(home: &Path) -> Result<(String, PathBuf, bool, u32, u32), String> {
    let output = run_limactl(&home.to_path_buf(), &["list", "--json"])?;
    if !output.status.success() {
        return Err(output_error(&output, "read Linux user configuration"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let instance: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("Could not parse Lima instance configuration: {error}"))?;
        if instance.get("name").and_then(|name| name.as_str()) != Some(POC_INSTANCE) {
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
        let uid = user
            .get("uid")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| "Lima does not report the guest user ID.".to_owned())?;
        let gid = user
            .get("gid")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| "Lima does not report the guest group ID.".to_owned())?;
        return Ok((
            name.to_owned(),
            PathBuf::from(guest_home),
            passwordless_sudo,
            uid,
            gid,
        ));
    }
    Err("The Orvex Linux VM has not been created.".into())
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
    if !workspaces.iter().any(|workspace| workspace.id == slug) {
        return Err("That workspace is not in the Orvex workspace list.".into());
    }

    let home = app_lima_home(app)?;
    if vm_state(&home)? != "running" {
        return Err("Start the Linux VM before opening a workspace terminal.".into());
    }
    let (guest_user, guest_home, passwordless_sudo, guest_uid, guest_gid) = guest_user(&home)?;
    if !network_enabled && !passwordless_sudo {
        return Err(
            "Offline terminals need passwordless sudo to create a Linux network namespace.".into(),
        );
    }
    if !network_enabled && (guest_uid == 0 || guest_gid == 0) {
        return Err("Offline terminals need a non-root Lima guest user.".into());
    }
    let guest_path = guest_home.join("orvex/workspaces").join(&slug);
    let guest_path_text = guest_path.to_string_lossy().into_owned();
    let path_check = run_limactl(
        &home,
        &["shell", POC_INSTANCE, "test", "-d", &guest_path_text],
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
            shell_quote(POC_INSTANCE),
        )
    } else {
        let guest_command = format!("cd {} && exec bash -l", shell_quote(&guest_path_text));
        format!(
            "exec {} shell --workdir {} {} sudo -n unshare --user --map-users=0:0:1 --map-users={guest_uid}:{guest_uid}:1 --map-groups=0:0:1 --map-groups={guest_gid}:{guest_gid}:1 --net --fork -- su -s /bin/bash -c {} {}\n",
            shell_quote(&limactl_text),
            shell_quote(&guest_path_text),
            shell_quote(POC_INSTANCE),
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

fn make_vm_status(state: String) -> VmStatus {
    VmStatus {
        state,
        instance_name: POC_INSTANCE,
        image_bytes: cached_lima_image_bytes(),
    }
}

fn start_poc_vm_blocking(app: &AppHandle) -> Result<VmStatus, String> {
    if !cfg!(target_os = "macos") {
        return Err("The Lima VM proof of concept currently runs on macOS only.".into());
    }
    let home = app_lima_home(app)?;
    let _ = app.emit(
        "vm-start-progress",
        VmProgress {
            phase: "checking",
            message: "Checking the VM and preparing its Linux image…".into(),
            downloaded_bytes: None,
        },
    );
    if vm_state(&home)? == "notCreated" {
        run_limactl_with_progress(
            app,
            &home,
            &[
                "start",
                "--name=orvex-poc",
                "--vm-type=vz",
                "--mount-none",
                "--containerd=none",
                "--cpus=2",
                "--memory=2",
                "--disk=8",
                "--progress",
                "--yes",
            ],
        )?;
    } else if vm_state(&home)? != "running" {
        run_limactl_with_progress(app, &home, &["start", POC_INSTANCE, "--progress", "--yes"])?;
    }
    let _ = app.emit(
        "vm-start-progress",
        VmProgress {
            phase: "ready",
            message: "Linux VM is ready.".into(),
            downloaded_bytes: None,
        },
    );
    Ok(make_vm_status(vm_state(&home)?))
}

fn run_poc_check_blocking(app: &AppHandle) -> Result<String, String> {
    let home = app_lima_home(app)?;
    if vm_state(&home)? != "running" {
        return Err("Start the Orvex VM before running the check.".into());
    }
    let output = run_limactl(&home, &["shell", POC_INSTANCE, "uname", "-a"])?;
    if !output.status.success() {
        return Err(output_error(&output, "run the Linux check"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn stop_poc_vm_blocking(app: &AppHandle, delete: bool) -> Result<VmStatus, String> {
    let home = app_lima_home(app)?;
    let state = vm_state(&home)?;
    if state == "notCreated" {
        return Ok(make_vm_status(state));
    }
    if delete {
        let output = run_limactl(&home, &["delete", "--force", POC_INSTANCE, "--yes"])?;
        if !output.status.success() {
            return Err(output_error(&output, "delete the Orvex VM"));
        }
        Ok(make_vm_status("notCreated".into()))
    } else {
        if state == "running" {
            let output = run_limactl(&home, &["stop", POC_INSTANCE, "--yes"])?;
            if !output.status.success() {
                return Err(output_error(&output, "stop the Orvex VM"));
            }
        }
        Ok(make_vm_status(vm_state(&home)?))
    }
}

fn create_workspace_blocking(
    app: &AppHandle,
    name: String,
    repo_url: Option<String>,
    allow_network: bool,
) -> Result<Workspace, String> {
    let name = name.trim().to_owned();
    let slug = workspace_slug(&name)?;
    let repo_url = repo_url
        .filter(|url| !url.trim().is_empty())
        .map(|url| validate_public_https_repo_url(&url))
        .transpose()?;
    if repo_url.is_some() && !allow_network {
        return Err("Approve network access to clone this repository.".into());
    }
    let mut workspaces = read_workspaces(app)?;
    if workspaces.iter().any(|workspace| workspace.id == slug) {
        return Err(format!("A workspace named “{name}” already exists."));
    }

    let home = app_lima_home(app)?;
    if vm_state(&home)? != "running" {
        return Err("Start the Linux VM before creating a workspace.".into());
    }

    const CREATE_WORKSPACE_SCRIPT: &str = r#"set -eu
root="$HOME/orvex/workspaces"
mkdir -p "$root"
if [ -e "$root/$2" ]; then
  printf 'Workspace path already exists. Choose another name.\n' >&2
  exit 17
fi
if [ -n "$1" ]; then
  GIT_TERMINAL_PROMPT=0 git clone --progress -- "$1" "$root/$2"
else
  mkdir "$root/$2"
  printf '# Empty workspace\n\nCreated by Orvex.\n' > "$root/$2/README.md"
fi
"#;
    let output = run_limactl(
        &home,
        &[
            "shell",
            POC_INSTANCE,
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
    };
    workspaces.push(workspace.clone());
    write_workspaces(app, &workspaces)?;
    Ok(workspace)
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
) -> Result<Workspace, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_workspace_blocking(&app, name, repo_url, allow_network)
    })
    .await
    .map_err(|error| format!("Workspace creation task failed: {error}"))?
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
            return Ok(make_vm_status("notCreated".into()));
        }
        let home = app_lima_home(&app)?;
        Ok(make_vm_status(vm_state(&home)?))
    })
    .await
    .map_err(|error| format!("VM status task failed: {error}"))?
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
            open_workspace_terminal,
            get_lima_status,
            install_lima,
            get_vm_status,
            start_poc_vm,
            run_poc_check,
            stop_poc_vm,
            delete_poc_vm
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orvex");
}
