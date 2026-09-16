// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

use crate::config::load::MosaicConfig;

pub fn save(config_path: &str, cfg: &MosaicConfig) -> anyhow::Result<()> {
    log::debug!("Save config: {}", config_path);
    if let Some(parent) = Path::new(config_path).parent() {
        // The mode only applies when the directory is created, so an existing
        // work directory is never locked down against non-root commands.
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    let mut content = String::from("[mosaic]\n");
    let mut keys: Vec<_> = cfg.mosaic.keys().collect();
    keys.sort();
    for k in keys {
        content.push_str(&format!("{} = {}\n", k, cfg.mosaic[k]));
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
        mosaic.insert("bundle_version".to_string(), "7".to_string());
        mosaic.insert("uid_range_start".to_string(), "5000".to_string());
        let cfg = MosaicConfig { mosaic };
        save(path_str, &cfg).unwrap();
        let loaded = load(path_str);
        assert_eq!(loaded.bundle_version(), "7");
        assert_eq!(loaded.uid_range().0, 5000);
    }
}
