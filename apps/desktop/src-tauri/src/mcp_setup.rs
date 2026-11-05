//! User-scoped MCP setup. Only the Orvex entry is added; conflicts are never replaced.
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use toml_edit::{Document, Item};

static SETUP_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Setup {
    provider: String,
    path: String,
    config: String,
    configured: bool,
    conflict: bool,
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| "Could not locate your home directory.".into())
}

fn target(provider: &str, home: &Path) -> Result<PathBuf, String> {
    let override_dir = |key: &str, fallback: PathBuf| -> Result<PathBuf, String> {
        let dir = std::env::var_os(key).map(PathBuf::from).unwrap_or(fallback);
        if !dir.is_absolute() {
            return Err(format!("{key} must be an absolute path."));
        }
        Ok(dir)
    };
    Ok(match provider {
        "codex" => override_dir("CODEX_HOME", home.join(".codex"))?.join("config.toml"),
        "claude" => home.join(".claude.json"),
        "copilot" => override_dir("COPILOT_HOME", home.join(".copilot"))?.join("mcp-config.json"),
        "pi" => override_dir("PI_CODING_AGENT_DIR", home.join(".pi/agent"))?.join("mcp.json"),
        "cursor" => home.join(".cursor/mcp.json"),
        "gemini" => home.join(".gemini/settings.json"),
        "other" => return Ok(PathBuf::new()),
        _ => return Err("Unknown MCP client.".into()),
    })
}

fn server(provider: &str) -> Result<Value, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut env = serde_json::Map::new();
    for key in if cfg!(windows) {
        &["USERPROFILE", "APPDATA"][..]
    } else {
        &["HOME", "XDG_DATA_HOME"][..]
    } {
        if let Ok(value) = std::env::var(key) {
            env.insert((*key).into(), json!(value));
        }
    }
    let mut value = json!({"command":executable,"args":["mcp"],"env":env});
    if provider == "copilot" {
        value["type"] = json!("local");
        value["tools"] = json!(["*"]);
        value["timeout"] = json!(600000);
    }
    Ok(value)
}

fn toml_config(server: &Value) -> String {
    // JSON strings are valid TOML basic strings, including Windows path escapes.
    let mut text = format!(
        "[mcp_servers.orvex]\ncommand = {}\nargs = [\"mcp\"]\n\n[mcp_servers.orvex.env]\n",
        server["command"]
    );
    for (key, value) in server["env"].as_object().unwrap() {
        text.push_str(&format!("{key} = {value}\n"));
    }
    text
}

fn merge(original: &str, provider: &str, server: &Value) -> Result<(String, bool, bool), String> {
    if provider == "codex" {
        let mut doc = original
            .parse::<Document>()
            .map_err(|e| format!("Invalid TOML; file unchanged: {e}"))?;
        let snippet = toml_config(server)
            .parse::<Document>()
            .map_err(|e| e.to_string())?;
        if let Some(servers) = doc.get("mcp_servers") {
            if !servers.is_table_like() {
                return Err("mcp_servers must be a table; file unchanged.".into());
            }
            if let Some(existing) = servers.get("orvex") {
                let same = existing.get("command").and_then(Item::as_str)
                    == server["command"].as_str()
                    && existing
                        .get("args")
                        .and_then(Item::as_array)
                        .map(|a| a.len() == 1 && a.get(0).and_then(|v| v.as_str()) == Some("mcp"))
                        .unwrap_or(false)
                    && existing
                        .get("env")
                        .map(|env| {
                            server["env"]
                                .as_object()
                                .unwrap()
                                .iter()
                                .all(|(k, v)| env.get(k).and_then(Item::as_str) == v.as_str())
                        })
                        .unwrap_or(false)
                    && existing.get("enabled").and_then(Item::as_bool) != Some(false);
                return Ok((original.into(), same, !same));
            }
        } else {
            doc["mcp_servers"] = Item::Table(toml_edit::Table::new());
        }
        doc["mcp_servers"]["orvex"] = snippet["mcp_servers"]["orvex"].clone();
        return Ok((doc.to_string(), false, false));
    }
    let mut doc: Value = if original.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(original).map_err(|e| format!("Invalid JSON; file unchanged: {e}"))?
    };
    let object = doc
        .as_object_mut()
        .ok_or("Configuration must be a JSON object; file unchanged.")?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("mcpServers must be an object; file unchanged.")?;
    if let Some(existing) = servers.get("orvex") {
        let same = server
            .as_object()
            .unwrap()
            .iter()
            .all(|(k, v)| existing.get(k) == Some(v))
            && existing.get("disabled") != Some(&json!(true))
            && existing.get("enabled") != Some(&json!(false));
        return Ok((original.into(), same, !same));
    }
    servers.insert("orvex".into(), server.clone());
    Ok((
        format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
        ),
        false,
        false,
    ))
}

fn read(path: &Path) -> Result<String, String> {
    if fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(
            "Configuration is a symbolic link. Use manual setup to edit its target.".into(),
        );
    }
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

pub(crate) fn inspect(provider: &str) -> Result<Setup, String> {
    let path = target(provider, &home()?)?;
    let server = server(provider)?;
    let config = if provider == "codex" {
        toml_config(&server)
    } else {
        serde_json::to_string_pretty(&json!({"mcpServers":{"orvex":server}}))
            .map_err(|e| e.to_string())?
    };
    let (_, configured, conflict) = if provider == "other" {
        (String::new(), false, false)
    } else {
        merge(&read(&path)?, provider, &server)?
    };
    Ok(Setup {
        provider: provider.into(),
        path: path.to_string_lossy().into(),
        config,
        configured,
        conflict,
    })
}

pub(crate) fn install(provider: &str) -> Result<Setup, String> {
    let _guard = SETUP_LOCK.lock().map_err(|e| e.to_string())?;
    if provider == "other" {
        return Err(
            "Choose a supported client, or copy the configuration for manual setup.".into(),
        );
    }
    let path = target(provider, &home()?)?;
    save(&path, provider, &server(provider)?)?;
    inspect(provider)
}

fn save(path: &Path, provider: &str, server: &Value) -> Result<(), String> {
    let original = read(path)?;
    let (updated, configured, conflict) = merge(&original, provider, server)?;
    if conflict {
        return Err("An Orvex entry already exists with different settings. Review it manually; nothing was changed.".into());
    }
    if configured {
        return Ok(());
    }
    fs::create_dir_all(path.parent().ok_or("Missing configuration directory")?)
        .map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    if path.exists() {
        let backup = path.with_extension(format!("orvex-backup-{stamp}"));
        write_new(&backup, original.as_bytes())?;
    }
    let temporary = path.with_extension(format!("orvex-{stamp}.tmp"));
    write_new(&temporary, updated.as_bytes())?;
    let result = (|| {
        if read(path)? != original {
            return Err(
                "Configuration changed during setup. Retry after the client finishes saving."
                    .into(),
            );
        }
        fs::rename(&temporary, &path).map_err(|e| format!("Could not save configuration: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_preserves_settings_and_refuses_conflicts() {
        let server = json!({"command":"/Applications/Orvex.app/orvex","args":["mcp"],"env":{"HOME":"/Users/test"}});
        for provider in ["claude", "copilot", "pi", "cursor", "gemini"] {
            let (text, _, _) = merge(
                r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
                provider,
                &server,
            )
            .unwrap();
            let parsed: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed["theme"], "dark");
            assert_eq!(parsed["mcpServers"]["other"]["command"], "other");
            assert!(merge(&text, provider, &server).unwrap().1);
            assert!(
                merge(r#"{"mcpServers":{"orvex":{}}}"#, provider, &server)
                    .unwrap()
                    .2
            );
        }
        assert!(merge("{broken", "pi", &server).is_err());
        assert!(merge(r#"{"mcpServers":[]}"#, "pi", &server).is_err());
    }
    #[test]
    fn saves_backup_and_is_idempotent_without_overwriting_conflicts() {
        let root = std::env::temp_dir().join(format!("orvex-setup-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        let original = "{\"theme\":\"dark\"}";
        fs::write(&path, original).unwrap();
        let server = json!({"command":"orvex", "args":["mcp"], "env":{}});
        save(&path, "gemini", &server).unwrap();
        let backups: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p != &path)
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
        save(&path, "gemini", &server).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(save(&path, "gemini", &json!({"command":"different"})).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        fs::write(&path, "{bad json").unwrap();
        assert!(save(&path, "gemini", &server).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{bad json");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn toml_keeps_comments_and_round_trips_windows_paths() {
        let server = json!({"command":"C:\\Program Files\\Orvex\\orvex.exe","args":["mcp"],"env":{"USERPROFILE":"C:\\Users\\Jane"}});
        let (text, _, _) = merge(
            "# keep me\nmodel = \"test\"\n[mcp_servers.other]\ncommand = \"other\"\n",
            "codex",
            &server,
        )
        .unwrap();
        assert!(text.contains("# keep me"));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(merge(&text, "codex", &server).unwrap().1);
        assert!(
            merge("[mcp_servers.orvex]\ncommand = 'other'", "codex", &server)
                .unwrap()
                .2
        );
        assert!(merge("mcp_servers = []", "codex", &server).is_err());
    }
}
