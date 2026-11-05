use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{Manager, Webview};
use tauri_plugin_updater::UpdaterExt;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UpdateChannel {
    Stable,
    Beta,
}

impl UpdateChannel {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Stable => {
                "https://github.com/mberrishdev/orvex/releases/latest/download/latest.json"
            }
            Self::Beta => {
                "https://github.com/mberrishdev/orvex/releases/download/updater-beta/latest.json"
            }
        }
    }
}

// Keep the resource in this webview so the updater plugin can download,
// verify, install, and close it through its existing permissions and API.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateMetadata {
    rid: tauri::ResourceId,
    current_version: String,
    version: String,
    body: Option<String>,
    raw_json: serde_json::Value,
}

#[tauri::command]
pub(crate) async fn check_for_app_update(
    webview: Webview,
    channel: UpdateChannel,
) -> Result<Option<UpdateMetadata>, String> {
    let updater = webview
        .updater_builder()
        .endpoints(vec![
            channel
                .endpoint()
                .parse()
                .map_err(|error| format!("Invalid update endpoint: {error}"))?,
        ])
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let update = updater.check().await.map_err(|error| match error {
        tauri_plugin_updater::Error::ReleaseNotFound => {
            "The selected release feed is unavailable. Try again later or choose another channel."
                .to_string()
        }
        error => error.to_string(),
    })?;
    Ok(update.map(|update| UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_restricted_to_published_release_feeds() {
        assert!(
            UpdateChannel::Stable
                .endpoint()
                .contains("/latest/download/")
        );
        assert!(
            UpdateChannel::Beta
                .endpoint()
                .contains("/download/updater-beta/")
        );
        assert!(serde_json::from_str::<UpdateChannel>("\"https://example.com\"").is_err());
    }
}
