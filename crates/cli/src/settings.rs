//! Per-device user choices. Core does not persist them: the CLI passes the
//! routing mode, the selected node and the ingress pins with every apply
//! (`docs/host-integration.md`, section 4.1).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CliError, Result};
use crate::paths::{ensure_private_dir, write_private_file};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingMode {
    /// The profile's rules, then its final action.
    #[default]
    Rules,
    /// Only the profile's baseline rules; everything else via the selected node.
    Global,
}

impl RoutingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RoutingMode::Rules => "rules",
            RoutingMode::Global => "global",
        }
    }

    pub fn parse(value: &str) -> Option<RoutingMode> {
        match value {
            "rules" => Some(RoutingMode::Rules),
            "global" => Some(RoutingMode::Global),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Unknown values fall back to rules rather than blocking start.
    #[serde(default, deserialize_with = "lenient_mode")]
    pub routing_mode: RoutingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_node_id: Option<String>,
    /// Node ID to pinned endpoint key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ingress_pins: BTreeMap<String, String>,
}

fn lenient_mode<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<RoutingMode, D::Error> {
    let value = Option::<String>::deserialize(d)?;
    Ok(value
        .as_deref()
        .and_then(RoutingMode::parse)
        .unwrap_or_default())
}

const MAX_SIZE: u64 = 64 << 10;

impl Settings {
    /// Reads the settings file; a missing file means defaults.
    pub fn load(path: &Path) -> Result<Settings> {
        let unreadable = |detail: String| {
            CliError::environment(
                "SETTINGS_UNAVAILABLE",
                format!("could not read the settings file: {detail}"),
            )
        };
        match fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_SIZE => return Err(unreadable("too large".into())),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
            Err(err) => return Err(unreadable(err.to_string())),
        }
        let data = fs::read(path).map_err(|e| unreadable(e.to_string()))?;
        serde_json::from_slice(&data).map_err(|e| unreadable(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let unwritable = |err: io::Error| {
            CliError::environment(
                "SETTINGS_UNAVAILABLE",
                format!("could not save the settings file: {err}"),
            )
        };
        let dir = path
            .parent()
            .ok_or_else(|| unwritable(io::Error::other("no parent directory")))?;
        ensure_private_dir(dir, true).map_err(unwritable)?;
        let mut data =
            serde_json::to_vec_pretty(self).map_err(|e| unwritable(io::Error::other(e)))?;
        data.push(b'\n');
        write_private_file(path, &data).map_err(unwritable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_means_rules_and_no_choices() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::load(&dir.path().join("missing/settings.json")).unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.routing_mode, RoutingMode::Rules);
    }

    #[test]
    fn round_trip_keeps_mode_selection_and_pins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ppvpn-cli/settings.json");
        let mut settings = Settings {
            routing_mode: RoutingMode::Global,
            selected_node_id: Some("hk-1".into()),
            ..Settings::default()
        };
        settings.ingress_pins.insert("hk-1".into(), "9002".into());
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"routing_mode\": \"global\""), "{text}");
    }

    #[test]
    fn unknown_values_and_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, br#"{"routing_mode":"direct","future":1}"#).unwrap();
        assert_eq!(
            Settings::load(&path).unwrap().routing_mode,
            RoutingMode::Rules
        );
    }

    #[test]
    fn corrupt_files_are_an_environment_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(Settings::load(&path).unwrap_err().exit_code(), 8);
    }
}
