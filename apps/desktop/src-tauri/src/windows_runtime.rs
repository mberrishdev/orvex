//! WSL 2 adapter for trusted development workspaces. Never changes global WSL
//! configuration or operates on distributions without a local ownership record.
#![cfg_attr(not(windows), allow(dead_code))]
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub(crate) const SETUP_HELP: &str = "Install or update WSL 2 in an Administrator PowerShell: wsl --install --no-distribution. Restart Windows if requested, then use Check again.";
const IMAGE: &str = "ubuntu-base-24.04.5-base-amd64.tar.gz";
const IMAGE_SHA256: &str = "e77b6f10c2590cef872b33ee9f635a0e3fd1f57fb074c0e52b5c7f56147a0c86";

pub(crate) fn executable() -> Result<PathBuf, String> {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32/wsl.exe"))
        .filter(|path| path.is_file())
        .ok_or_else(|| SETUP_HELP.to_owned())
}

pub(crate) fn command() -> Result<Command, String> {
    #[allow(unused_mut)]
    let mut command = Command::new(executable()?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW; terminals override this.
    }
    Ok(command)
}

// WSL management commands return UTF-16LE on some releases and UTF-8 on others.
// Guest command output is always returned untouched.
fn management_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.iter().skip(1).step_by(2).any(|b| *b == 0) {
        let bytes = bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes);
        String::from_utf16_lossy(
            &bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}
fn checked(mut command: Command) -> Result<Output, String> {
    let output = command
        .output()
        .map_err(|e| format!("Could not run WSL: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "WSL failed: {} {}",
            management_text(&output.stderr).trim(),
            management_text(&output.stdout).trim()
        ));
    }
    Ok(output)
}
fn management(args: &[&str]) -> Result<Output, String> {
    let mut cmd = command()?;
    cmd.args(args);
    checked(cmd)
}
pub(crate) fn version() -> Result<String, String> {
    let version = management(&["--version"])?;
    management(&["--status"])?;
    Ok(management_text(&version.stdout).trim().to_owned())
}
fn distributions(running: bool) -> Result<Vec<String>, String> {
    let args = if running {
        vec!["--list", "--quiet", "--running"]
    } else {
        vec!["--list", "--quiet"]
    };
    Ok(management_text(&management(&args)?.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}
fn validate_instance(instance: &str) -> Result<(), String> {
    if !instance.starts_with("orvex-")
        || instance.len() > 80
        || !instance
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid Orvex WSL distribution name.".into());
    }
    Ok(())
}
fn owned(home: &Path, instance: &str) -> Result<(), String> {
    validate_instance(instance)?;
    if std::fs::read_to_string(home.join(instance).join("orvex-owner"))
        .ok()
        .as_deref()
        != Some(instance)
    {
        return Err("This WSL distribution is not owned by Orvex; refusing to modify it.".into());
    }
    Ok(())
}
pub(crate) fn shell(home: &Path, instance: &str, workdir: &str) -> Result<Command, String> {
    owned(home, instance)?;
    let mut cmd = command()?;
    cmd.args([
        "--distribution",
        instance,
        "--user",
        "orvex",
        "--cd",
        workdir,
        "--exec",
        "env",
        "HOME=/home/orvex",
    ]);
    Ok(cmd)
}
fn root(instance: &str, script: &str) -> Result<Output, String> {
    let mut cmd = command()?;
    cmd.args([
        "--distribution",
        instance,
        "--user",
        "root",
        "--cd",
        "/",
        "--exec",
        "sh",
        "-c",
        script,
    ]);
    checked(cmd)
}

fn create(home: &Path, instance: &str) -> Result<Output, String> {
    validate_instance(instance)?;
    if distributions(false)?
        .iter()
        .any(|name| name.eq_ignore_ascii_case(instance))
    {
        return Err(
            "A WSL distribution with this name already exists; it will not be replaced.".into(),
        );
    }
    // This first release ships x64 installers. Do not silently emulate on ARM.
    if std::env::consts::ARCH != "x86_64" {
        return Err("The Windows runtime currently requires Windows x64.".into());
    }
    let directory = home.join(instance);
    std::fs::create_dir(&directory).map_err(|e| format!("Could not reserve WSL storage: {e}"))?;
    std::fs::write(directory.join("orvex-owner"), instance).map_err(|e| e.to_string())?;
    let image = directory.join(IMAGE);
    // PowerShell receives the path as an environment variable, never as script text.
    let mut download = Command::new("powershell.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        download.creation_flags(0x08000000);
    }
    download
        .env("ORVEX_IMAGE_PATH", &image)
        .env("ORVEX_IMAGE_HASH", IMAGE_SHA256);
    download.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &format!(r#"$ErrorActionPreference='Stop'; Invoke-WebRequest -UseBasicParsing -TimeoutSec 600 -Uri 'https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/{IMAGE}' -OutFile $env:ORVEX_IMAGE_PATH; if ((Get-FileHash -Algorithm SHA256 -LiteralPath $env:ORVEX_IMAGE_PATH).Hash -ne $env:ORVEX_IMAGE_HASH) {{ throw 'Ubuntu image checksum mismatch' }}"#)]);
    checked(download)?;
    let mut import = command()?;
    import
        .args(["--import", instance])
        .arg(directory.join("disk"))
        .arg(&image)
        .args(["--version", "2"]);
    checked(import)?;
    let _ = std::fs::remove_file(image);
    root(instance, PROVISION)?;
    management(&["--terminate", instance])?;
    start(home, instance)
}
const PROVISION: &str = r#"set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y ca-certificates curl git sudo util-linux python3 systemd systemd-sysv iproute2
id orvex >/dev/null 2>&1 || useradd --create-home --shell /bin/bash orvex
printf 'orvex ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/orvex
chmod 440 /etc/sudoers.d/orvex
cat > /etc/wsl.conf <<'CONFIG'
[boot]
systemd=true
[automount]
enabled=false
mountFsTab=false
[interop]
enabled=false
appendWindowsPath=false
[user]
default=orvex
CONFIG
"#;
fn start(home: &Path, instance: &str) -> Result<Output, String> {
    let mut cmd = shell(home, instance, "/home/orvex")?;
    cmd.args([
        "sh",
        "-c",
        "nohup sleep infinity >/dev/null 2>&1 </dev/null &",
    ]);
    checked(cmd)
}

/// Translate the shared controller's lifecycle/shell/copy operations to WSL.
pub(crate) fn run(home: &Path, args: &[&str]) -> Result<Output, String> {
    match args {
        ["list", "--json"] => {
            let mut result = management(&["--list", "--quiet"])?;
            let running = distributions(true)?;
            let records = management_text(&result.stdout).lines().map(str::trim).filter(|s| !s.is_empty()).map(|name| {
                serde_json::json!({"name":name,"status":if running.iter().any(|n|n == name) {"Running"} else {"Stopped"},
                    "config":{"user":{"name":"orvex","home":"/home/orvex","passwordlessSudo":true}}}).to_string()
            }).collect::<Vec<_>>();
            result.stdout = records.join("\n").into_bytes();
            Ok(result)
        }
        ["start", name, ..] if name.starts_with("--name=") => create(home, &name[7..]),
        ["start", instance, ..] => start(home, instance),
        ["stop", instance, ..] => {
            owned(home, instance)?;
            management(&["--terminate", instance])
        }
        ["delete", "--force", instance, ..] => {
            owned(home, instance)?;
            let output = if distributions(false)?
                .iter()
                .any(|n| n.eq_ignore_ascii_case(instance))
            {
                management(&["--unregister", instance])?
            } else {
                management(&["--version"])?
            };
            std::fs::remove_dir_all(home.join(instance)).map_err(|e| e.to_string())?;
            Ok(output)
        }
        ["shell", rest @ ..] => {
            let (workdir, rest) = if let ["--workdir", path, rest @ ..] = rest {
                (*path, rest)
            } else {
                ("/home/orvex", rest)
            };
            let (instance, argv) = rest.split_first().ok_or("Missing WSL distribution.")?;
            let mut cmd = shell(home, instance, workdir)?;
            cmd.args(argv);
            cmd.output()
                .map_err(|e| format!("Could not run WSL command: {e}"))
        }
        ["copy", source, destination] => {
            let (instance, path) = destination
                .split_once(':')
                .ok_or("Invalid guest destination.")?;
            let mut cmd = shell(home, instance, "/home/orvex")?;
            cmd.args(["sh", "-c", &format!("cat > {}", crate::shell_quote(path))]);
            cmd.stdin(Stdio::from(
                std::fs::File::open(source).map_err(|e| e.to_string())?,
            ));
            checked(cmd)
        }
        _ => Err(format!(
            "Unsupported WSL runtime operation: {:?}",
            args.first()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_wsl_management_output_without_changing_guest_output() {
        let utf16 = "Ubuntu\r\norvex-test\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(management_text(&utf16), "Ubuntu\r\norvex-test\r\n");
        assert_eq!(management_text(b"orvex-test\n"), "orvex-test\n");
    }
    #[cfg(windows)]
    #[test]
    fn controller_instances_use_the_owned_wsl_namespace() {
        let instance = crate::workspace_instance_name("project-with-spaces");
        validate_instance(&instance).unwrap();
        assert_eq!(
            instance,
            crate::workspace_instance_name("project-with-spaces")
        );
    }
    #[test]
    fn refuses_unowned_distributions_and_path_traversal() {
        let dir = std::env::temp_dir().join("orvex-wsl-unowned");
        assert!(owned(&dir, "Ubuntu").is_err());
        assert!(owned(&dir, "orvex-../../Ubuntu").is_err());
        assert!(owned(&dir, "orvex-missing").is_err());
        // These must fail before discovering or launching WSL, on every host.
        assert!(run(&dir, &["stop", "Ubuntu", "--yes"]).is_err());
        assert!(run(&dir, &["delete", "--force", "orvex-missing", "--yes"]).is_err());
        assert!(!dir.exists());
    }
}
