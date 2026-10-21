//! The app's own settings (#401 step 3): appearance, language and the API
//! override, in `settings.json` in the data directory. They belong to the
//! UI, not to the client, which keeps its own state (routing mode, node)
//! elsewhere. A missing or damaged file reads as the defaults.

use serde::{Deserialize, Serialize};

use crate::api::{UiAppearance, UiLanguage, UiSettings};
use crate::bridge;

const FILE: &str = "settings.json";

/// The file's shape. Values are plain strings so that an unknown one (from
/// a later version) falls back to its default instead of failing the file.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    appearance: String,
    language: String,
    api_override: String,
}

fn path(data_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(data_dir).join(FILE)
}

fn read(data_dir: &str) -> Saved {
    let path = path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "flutter settings unreadable; defaults");
            Saved::default()
        }),
        Err(_) => Saved::default(),
    }
}

pub(crate) fn load() -> Result<UiSettings, String> {
    let (data_dir, _) = bridge::dirs()?;
    let saved = read(&data_dir);
    Ok(UiSettings {
        appearance: match saved.appearance.as_str() {
            "light" => UiAppearance::Light,
            "dark" => UiAppearance::Dark,
            _ => UiAppearance::System,
        },
        language: match saved.language.as_str() {
            "zh" => UiLanguage::Chinese,
            "en" => UiLanguage::English,
            _ => UiLanguage::System,
        },
        api_override: saved.api_override,
    })
}

/// Writes the file whole (a temporary file, then a rename). The override
/// must be empty or an http(s) URL.
pub(crate) fn save(settings: UiSettings) -> Result<(), String> {
    let api_override = settings.api_override.trim().trim_end_matches('/');
    if !api_override.is_empty()
        && !api_override.starts_with("https://")
        && !api_override.starts_with("http://")
    {
        return Err(format!("not an http(s) URL: {api_override}"));
    }
    let saved = Saved {
        appearance: match settings.appearance {
            UiAppearance::System => "system",
            UiAppearance::Light => "light",
            UiAppearance::Dark => "dark",
        }
        .into(),
        language: match settings.language {
            UiLanguage::System => "system",
            UiLanguage::Chinese => "zh",
            UiLanguage::English => "en",
        }
        .into(),
        api_override: api_override.into(),
    };
    let (data_dir, _) = bridge::dirs()?;
    let path = path(&data_dir);
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
    std::fs::write(&temp, json).map_err(|error| format!("{}: {error}", temp.display()))?;
    std::fs::rename(&temp, &path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The saved API override, if any; read once when the client starts.
pub(crate) fn api_override(data_dir: &str) -> Option<String> {
    Some(read(data_dir).api_override).filter(|base| !base.is_empty())
}
