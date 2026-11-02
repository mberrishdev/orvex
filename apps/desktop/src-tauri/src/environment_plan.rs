//! Persistent, revisioned review artifacts. Approval never executes setup.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Item {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) enabled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Plan {
    pub(crate) id: String,
    pub(crate) revision: u64,
    pub(crate) status: String,
    pub(crate) approved_revision: Option<u64>,
    pub(crate) source: Value,
    pub(crate) commit: String,
    pub(crate) items: Vec<Item>,
    pub(crate) inspection: Value,
    pub(crate) execution_authorized: bool,
}

pub(crate) struct Store(PathBuf);

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate(items: &[Item]) -> Result<(), String> {
    if items.len() > 128 {
        return Err("An environment plan supports up to 128 items.".into());
    }
    let mut ids = BTreeSet::new();
    for item in items {
        if !identifier(&item.id) || !ids.insert(&item.id) {
            return Err("Every plan item needs a unique identifier (letters, digits, hyphens or underscores).".into());
        }
        if ![
            "runtime",
            "tool",
            "service",
            "containerImage",
            "configuration",
            "externalConfiguration",
        ]
        .contains(&item.kind.as_str())
        {
            return Err(format!("Unsupported plan item kind: {}", item.kind));
        }
        if item.name.trim().is_empty()
            || item.name.len() > 256
            || item.name.chars().any(char::is_control)
        {
            return Err(
                "Item names must contain 1–256 characters without control characters.".into(),
            );
        }
        if item
            .version
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
        {
            return Err(
                "Versions must contain 1–128 characters without control characters, or null."
                    .into(),
            );
        }
    }
    Ok(())
}

impl Store {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self(data_dir.join("environment-plans"))
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !id.starts_with("plan-") || !identifier(id) {
            return Err("Invalid environment plan ID.".into());
        }
        Ok(self.0.join(format!("{id}.json")))
    }
    fn save(&self, plan: &Plan) -> Result<(), String> {
        let data = serde_json::to_vec_pretty(plan).map_err(|e| e.to_string())?;
        if data.len() > 2 * 1024 * 1024 {
            return Err("Environment plan exceeds 2 MiB.".into());
        }
        let temporary = self
            .0
            .join(format!(".{}-{}.tmp", plan.id, std::process::id()));
        std::fs::write(&temporary, data).map_err(|e| e.to_string())?;
        if let Err(error) = std::fs::rename(&temporary, self.path(&plan.id)?) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
    pub(crate) fn get(&self, id: &str) -> Result<Plan, String> {
        let path = self.path(id)?;
        if std::fs::metadata(&path)
            .map_err(|e| format!("Could not read environment plan: {e}"))?
            .len()
            > 2 * 1024 * 1024
        {
            return Err("Environment plan exceeds 2 MiB.".into());
        }
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid environment plan: {e}"))
    }
    pub(crate) fn create(&self, source: Value, inspection: Value) -> Result<Plan, String> {
        let _lock = crate::workspace_store::MutationLock::acquire(&self.0)?;
        let commit = inspection["commit"]
            .as_str()
            .ok_or("Inspection did not report its commit.")?
            .to_owned();
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for need in inspection["needs"]
            .as_array()
            .ok_or("Inspection did not return needs.")?
        {
            let kind = need["kind"].as_str().unwrap_or("tool").to_owned();
            let name = need["name"]
                .as_str()
                .ok_or("A detected need has no name.")?
                .to_owned();
            let version = need["versionRequirement"].as_str().map(str::to_owned);
            if !seen.insert((kind.clone(), name.clone(), version.clone())) {
                continue;
            }
            items.push(Item {
                id: format!("need-{}", items.len() + 1),
                kind,
                name,
                version,
                enabled: true,
            });
        }
        validate(&items)?;
        let id = format!(
            "plan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        let plan = Plan {
            id,
            revision: 1,
            status: "draft".into(),
            approved_revision: None,
            source,
            commit,
            items,
            inspection,
            execution_authorized: false,
        };
        self.save(&plan)?;
        Ok(plan)
    }
    pub(crate) fn update(&self, id: &str, expected: u64, items: Vec<Item>) -> Result<Plan, String> {
        validate(&items)?;
        let _lock = crate::workspace_store::MutationLock::acquire(&self.0)?;
        let mut plan = self.get(id)?;
        if plan.revision != expected {
            return Err(format!(
                "Stale plan revision. Read revision {} before editing.",
                plan.revision
            ));
        }
        plan.items = items;
        plan.revision += 1;
        plan.status = "draft".into();
        plan.approved_revision = None;
        plan.execution_authorized = false;
        self.save(&plan)?;
        Ok(plan)
    }
    pub(crate) fn approve(
        &self,
        id: &str,
        expected: u64,
        confirmed: bool,
        current_commit: &str,
    ) -> Result<Plan, String> {
        if !confirmed {
            return Err("Show the current plan to the user and obtain explicit acceptance before passing confirmed=true.".into());
        }
        let _lock = crate::workspace_store::MutationLock::acquire(&self.0)?;
        let mut plan = self.get(id)?;
        if plan.revision != expected {
            return Err(format!(
                "Stale plan revision. Show revision {} to the user before approval.",
                plan.revision
            ));
        }
        if plan.commit != current_commit {
            return Err(
                "The project commit changed since inspection. Create and review a fresh plan."
                    .into(),
            );
        }
        plan.status = "approved".into();
        plan.approved_revision = Some(plan.revision);
        plan.execution_authorized = false;
        self.save(&plan)?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn approval_is_revisioned_and_edits_require_renewed_acceptance() {
        let root = std::env::temp_dir().join(format!(
            "orvex-plan-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::new(&root);
        let plan=store.create(json!({"projectPath":"/fixture"}),json!({"commit":"one","needs":[{"kind":"service","name":"PostgreSQL"},{"kind":"service","name":"PostgreSQL"}]})).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.status, "draft");
        assert!(store.approve(&plan.id, 1, false, "one").is_err());
        assert!(store.approve(&plan.id, 1, true, "two").is_err());
        let approved = store.approve(&plan.id, 1, true, "one").unwrap();
        assert_eq!(approved.approved_revision, Some(1));
        assert!(!approved.execution_authorized);
        let mut items = approved.items;
        items[0].version = Some("17".into());
        items.push(Item {
            id: "custom-redis".into(),
            kind: "service".into(),
            name: "Redis".into(),
            version: None,
            enabled: true,
        });
        let edited = store.update(&plan.id, 1, items).unwrap();
        assert_eq!(edited.revision, 2);
        assert_eq!(edited.status, "draft");
        assert_eq!(edited.approved_revision, None);
        assert!(store.approve(&plan.id, 1, true, "one").is_err());
        assert!(store.update(&plan.id, 1, Vec::new()).is_err());
        assert_eq!(Store::new(&root).get(&plan.id).unwrap().items.len(), 2);
        assert!(store.get("plan-../../escape").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
