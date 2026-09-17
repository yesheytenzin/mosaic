// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use crate::config::{Defaults, CONFIG_KEYS};

#[derive(Debug, Clone)]
pub struct MosaicConfig {
    pub mosaic: HashMap<String, String>,
}

impl MosaicConfig {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.mosaic.get(key)
    }

    /// Where a published bundle is fetched from: the environment, then the config
    /// file, then the project's own releases.
    pub fn bundle_channel(&self) -> String {
        if let Ok(channel) = std::env::var("MOSAIC_BUNDLE_CHANNEL") {
            if !channel.is_empty() {
                return channel;
            }
        }
        self.mosaic
            .get("bundle_channel")
            .cloned()
            .unwrap_or_else(|| "0".to_string())
    }

    pub fn bundle_version(&self) -> String {
        // Overridable like the channel, for the same reason: a bundle packed on one
        // machine has the version it was installed under, and the machine fetching it
        // has to ask for that name.
        if let Ok(version) = std::env::var("MOSAIC_BUNDLE_VERSION") {
            if !version.is_empty() {
                return version;
            }
        }
        self.mosaic
            .get("bundle_version")
            .cloned()
            .unwrap_or_else(|| "local".to_string())
    }

    pub fn uid_range(&self) -> (u32, u32) {
        let defaults = Defaults::new();
        let start = self
            .mosaic
            .get("uid_range_start")
            .and_then(|v| v.parse().ok())
            .unwrap_or(defaults.uid_range_start);
        let end = self
            .mosaic
            .get("uid_range_end")
            .and_then(|v| v.parse().ok())
            .unwrap_or(defaults.uid_range_end);
        (start, end)
    }
}

/// Minimal INI reader. Kept hand rolled so quoting and comment behaviour match
/// the config files the project has always written.
fn parse_ini(path: &str) -> HashMap<String, HashMap<String, String>> {
    let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return result,
    };
    let mut current_section = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].trim().to_string();
            result.entry(current_section.clone()).or_default();
            continue;
        }
        if current_section.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=').or_else(|| line.split_once(':')) {
            result
                .entry(current_section.clone())
                .or_default()
                .insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    result
}

pub fn load(config_path: &str) -> MosaicConfig {
    let defaults = Defaults::new();
    let mut mosaic = HashMap::new();

    let ini = parse_ini(config_path);
    if let Some(section) = ini.get("mosaic") {
        for (k, v) in section {
            mosaic.insert(k.clone(), v.clone());
        }
    }

    for key in CONFIG_KEYS {
        if !mosaic.contains_key(*key) {
            let value = match *key {
                "bundle_channel" => defaults.bundle_channel.clone(),
                "bundle_version" => defaults.bundle_version.clone(),
                "uid_range_start" => defaults.uid_range_start.to_string(),
                "uid_range_end" => defaults.uid_range_end.to_string(),
                _ => continue,
            };
            mosaic.insert(key.to_string(), value);
        }
    }

    // Drop anything that is not a current key, so an old file cannot resurrect
    // container-era settings.
    let stale: Vec<String> = mosaic
        .keys()
        .filter(|k| !CONFIG_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    for key in stale {
        mosaic.remove(&key);
    }

    MosaicConfig { mosaic }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_missing_file_returns_defaults() {
        let cfg = load("/nonexistent/path/mosaic.cfg");
        assert_eq!(cfg.bundle_version(), "local");
        assert_eq!(cfg.uid_range(), (5000, 5999));
    }

    #[test]
    fn load_reads_overrides() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "[mosaic]").unwrap();
        writeln!(tmp, "bundle_version = 42").unwrap();
        writeln!(tmp, "uid_range_start = 6000").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let cfg = load(&path);
        assert_eq!(cfg.bundle_version(), "42");
        assert_eq!(cfg.uid_range().0, 6000);
    }

    #[test]
    fn load_drops_stale_container_keys() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "[mosaic]").unwrap();
        writeln!(tmp, "rootfs = /var/lib/mosaic/rootfs").unwrap();
        writeln!(tmp, "mount_overlays = True").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let cfg = load(&path);
        assert!(cfg.get("rootfs").is_none());
        assert!(cfg.get("mount_overlays").is_none());
    }
}
