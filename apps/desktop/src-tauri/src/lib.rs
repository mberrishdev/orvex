use serde::Serialize;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
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

#[derive(Serialize)]
struct Workspace {
    id: String,
    name: String,
    branch: String,
    status: String,
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

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        name: "Orvex",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn list_workspaces() -> Vec<Workspace> {
    Vec::new()
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
