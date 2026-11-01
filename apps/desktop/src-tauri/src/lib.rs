use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

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
    let brew = find_executable("brew")
        .ok_or_else(|| "Homebrew was not found. Install Homebrew, then retry from Orvex.".to_owned())?;
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
        return Err("Homebrew finished, but Orvex could not find limactl. Restart Orvex and check again.".into());
    }
    Ok(status)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            list_workspaces,
            get_lima_status,
            install_lima
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orvex");
}
