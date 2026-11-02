//! Local MCP session presence, based on a negotiated client and live server PID.
//! Never reads provider credentials or guesses client presence from installed CLIs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Client {
    id: String,
    name: String,
    version: String,
    process_id: u32,
    connected_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    client: Client,
    process_identity: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    clients: Vec<Client>,
    ignored_records: usize,
}
pub(crate) struct Connection(PathBuf);

fn identity(pid: u32) -> Result<String, String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || text.is_empty() {
        return Err("MCP server process is no longer running.".into());
    }
    Ok(text)
}
fn label(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect()
}

impl Connection {
    pub(crate) fn register(data: &Path, info: &Value) -> Result<Self, String> {
        let directory = data.join("mcp-connections");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?;
        let id = format!("client-{}-{}", std::process::id(), now.as_nanos());
        let path = directory.join(format!("{id}.json"));
        let record = Record {
            client: Client {
                id,
                name: label(info.get("name")),
                version: label(info.get("version")),
                process_id: std::process::id(),
                connected_at: now.as_secs(),
            },
            process_identity: identity(std::process::id())?,
        };
        let temporary = path.with_extension("tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, &path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub(crate) fn status(data: &Path) -> Result<Status, String> {
    let entries = match std::fs::read_dir(data.join("mcp-connections")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Status {
                clients: Vec::new(),
                ignored_records: 0,
            });
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut clients = Vec::new();
    let mut ignored = 0;
    for entry in entries.take(256) {
        let Ok(entry) = entry else {
            ignored += 1;
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            || entry.metadata().map(|m| m.len() > 16384).unwrap_or(true)
        {
            ignored += 1;
            continue;
        }
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok());
        if let Some(record) = record {
            if identity(record.client.process_id).ok().as_deref()
                == Some(record.process_identity.as_str())
            {
                clients.push(record.client);
            }
        } else {
            ignored += 1;
        }
    }
    clients.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.connected_at.cmp(&b.connected_at))
    });
    Ok(Status {
        clients,
        ignored_records: ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn live_handshake_records_are_removed_on_disconnect_and_stale_pids_are_not_connected() {
        let root = std::env::temp_dir().join(format!("orvex-mcp-presence-{}", std::process::id()));
        assert!(status(&root).unwrap().clients.is_empty());
        let connection =
            Connection::register(&root, &json!({"name":"codex","version":"fixture"})).unwrap();
        assert_eq!(status(&root).unwrap().clients[0].name, "codex");
        let mut record: Record =
            serde_json::from_slice(&std::fs::read(&connection.0).unwrap()).unwrap();
        record.process_identity = "stale identity".into();
        std::fs::write(&connection.0, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(status(&root).unwrap().clients.is_empty());
        drop(connection);
        assert!(status(&root).unwrap().clients.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
