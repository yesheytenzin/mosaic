// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

use crate::config::load::MosaicConfig;

pub fn save(config_path: &str, cfg: &MosaicConfig) -> anyhow::Result<()> {
    log::debug!("Save config: {}", config_path);
    if let Some(parent) = Path::new(config_path).parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(parent, perms);
        }
    }
    let mut content = String::new();
    content.push_str("[mosaic]\n");
    let mut keys: Vec<_> = cfg.mosaic.keys().collect();
    keys.sort();
    for k in keys {
        let v = &cfg.mosaic[k];
        content.push_str(&format!("{} = {}\n", k, v));
    }
    if !cfg.properties.is_empty() {
        content.push_str("\n[properties]\n");
        let mut pkeys: Vec<_> = cfg.properties.keys().collect();
        pkeys.sort();
        for k in pkeys {
            let v = &cfg.properties[k];
            content.push_str(&format!("{} = {}\n", k, v));
        }
    }
    std::fs::write(config_path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load::load;
    use std::collections::HashMap;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mosaic.cfg");
        let path_str = path.to_str().unwrap();
        let mut mosaic = HashMap::new();
        mosaic.insert("arch".to_string(), "arm64".to_string());
        mosaic.insert("system_datetime".to_string(), "12345".to_string());
        let mut properties = HashMap::new();
        properties.insert("ro.sf.lcd_density".to_string(), "240".to_string());
        let cfg = MosaicConfig { mosaic, properties };
        save(path_str, &cfg).unwrap();
        let loaded = load(path_str);
        assert_eq!(loaded.mosaic.get("arch").unwrap(), "arm64");
        assert_eq!(loaded.properties.get("ro.sf.lcd_density").unwrap(), "240");
    }
}
