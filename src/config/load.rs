// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::path::Path;

use crate::config::{channels_defaults, CHANNELS_CONFIG_KEYS, CONFIG_KEYS};

#[derive(Debug, Clone)]
pub struct MosaicConfig {
    pub mosaic: HashMap<String, String>,
    pub properties: HashMap<String, String>,
}

impl MosaicConfig {
    pub fn get(&self, section: &str, key: &str) -> Option<&String> {
        match section {
            "mosaic" => self.mosaic.get(key),
            "properties" => self.properties.get(key),
            _ => None,
        }
    }
}

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
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_string();
            let value = line[eq + 1..].trim().to_string();
            if current_section.is_empty() {
                continue;
            }
            result
                .entry(current_section.clone())
                .or_default()
                .insert(key, value);
        } else if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            if current_section.is_empty() {
                continue;
            }
            result
                .entry(current_section.clone())
                .or_default()
                .insert(key, value);
        }
    }
    result
}

pub fn load(config_path: &str) -> MosaicConfig {
    let defaults = crate::config::Defaults::new();
    let mut mosaic = HashMap::new();
    let mut properties = HashMap::new();

    let ini = parse_ini(config_path);

    if let Some(section) = ini.get("mosaic") {
        for (k, v) in section {
            mosaic.insert(k.clone(), v.clone());
        }
    }

    // Apply defaults for missing config_keys
    for key in CONFIG_KEYS {
        if !mosaic.contains_key(*key) {
            let default_val = match *key {
                "arch" => defaults.arch.clone(),
                "images_path" => defaults.images_path.clone(),
                "vendor_type" => defaults.vendor_type.clone(),
                "system_datetime" => defaults.system_datetime.clone(),
                "vendor_datetime" => defaults.vendor_datetime.clone(),
                "suspend_action" => defaults.suspend_action.clone(),
                "mount_overlays" => defaults.mount_overlays.clone(),
                "auto_adb" => defaults.auto_adb.clone(),
                _ => continue,
            };
            mosaic.insert(key.to_string(), default_val);
        }
    }

    // Remove unconfigurable keys that were saved in old configs
    let all_default_keys = [
        "arch",
        "work",
        "vendor_type",
        "system_datetime",
        "vendor_datetime",
        "preinstalled_images_paths",
        "suspend_action",
        "mount_overlays",
        "auto_adb",
        "container_xdg_runtime_dir",
        "container_wayland_display",
        "images_path",
        "rootfs",
        "overlay",
        "overlay_rw",
        "overlay_work",
        "data",
        "lxc",
        "host_perms",
        "container_pulse_runtime_path",
    ];
    for key in all_default_keys {
        if !CONFIG_KEYS.contains(&key) && mosaic.contains_key(key) {
            log::debug!(
                "Ignored unconfigurable and possibly outdated default value from config: {}",
                mosaic[key]
            );
            mosaic.remove(key);
        }
    }

    if let Some(section) = ini.get("properties") {
        for (k, v) in section {
            properties.insert(k.clone(), v.clone());
        }
    }

    MosaicConfig { mosaic, properties }
}

#[derive(Debug, Clone)]
pub struct ChannelsConfig {
    pub channels: HashMap<String, String>,
}

pub fn load_channels() -> ChannelsConfig {
    let defaults = channels_defaults();
    let config_path = defaults.get("config_path").unwrap().clone();
    let mut channels = HashMap::new();

    let ini = if Path::new(&config_path).is_file() {
        parse_ini(&config_path)
    } else {
        HashMap::new()
    };

    if let Some(section) = ini.get("channels") {
        for (k, v) in section {
            channels.insert(k.clone(), v.clone());
        }
    }

    for key in CHANNELS_CONFIG_KEYS {
        if !channels.contains_key(*key) {
            if let Some(v) = defaults.get(*key) {
                channels.insert(key.to_string(), v.clone());
            }
        }
    }

    for k in defaults.keys() {
        if !CHANNELS_CONFIG_KEYS.contains(&k.as_str()) && channels.contains_key(k) {
            log::debug!(
                "Ignored unconfigurable and possibly outdated default value from config: {}",
                channels[k]
            );
            channels.remove(k);
        }
    }

    ChannelsConfig { channels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_missing_file_returns_defaults() {
        let cfg = load("/nonexistent/path/mosaic.cfg");
        assert_eq!(cfg.mosaic.get("arch").unwrap(), "arm64");
        assert_eq!(cfg.mosaic.get("system_datetime").unwrap(), "0");
        assert!(cfg.properties.is_empty());
    }

    #[test]
    fn load_channels_missing_file_returns_defaults() {
        let cfg = load_channels();
        assert_eq!(
            cfg.channels.get("system_channel").unwrap(),
            "https://ota.waydro.id/system"
        );
        assert_eq!(cfg.channels.get("rom_type").unwrap(), "lineage");
    }

    #[test]
    fn load_preserves_properties_section() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "[mosaic]").unwrap();
        writeln!(tmp, "arch = x86_64").unwrap();
        writeln!(tmp, "[properties]").unwrap();
        writeln!(tmp, "ro.sf.lcd_density = 320").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let cfg = load(&path);
        assert_eq!(cfg.mosaic.get("arch").unwrap(), "x86_64");
        assert_eq!(cfg.properties.get("ro.sf.lcd_density").unwrap(), "320");
    }

    #[test]
    fn load_removes_unconfigurable_keys() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "[mosaic]").unwrap();
        writeln!(tmp, "arch = arm64").unwrap();
        writeln!(tmp, "work = /custom/work").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let cfg = load(&path);
        assert!(!cfg.mosaic.contains_key("work"));
    }
}
