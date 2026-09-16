// SPDX-License-Identifier: GPL-3.0-or-later

//! The package registry. The broker owns it, and it is the single source of
//! truth for what is installed. Writes go through a temporary file and a rename
//! so a crash mid-write cannot corrupt it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    /// The app's own system UID (ADR-0007).
    pub uid: u32,
    pub data_dir: String,
    pub apk: String,
    pub installed_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub packages: BTreeMap<String, Package>,
}

impl Registry {
    fn path(work: &str) -> String {
        format!("{}/registry.json", work)
    }

    pub fn load(work: &str) -> Self {
        let path = Self::path(work);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<BTreeMap<String, Package>>(&text) {
                Ok(packages) => Self { packages },
                Err(e) => {
                    log::warn!("Ignoring unreadable registry {}: {}", path, e);
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, work: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(work)?;
        let path = Self::path(work);
        let tmp = format!("{}.tmp", path);
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.packages)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn add(&mut self, package: Package) {
        self.packages.insert(package.name.clone(), package);
    }

    pub fn remove(&mut self, name: &str) -> Option<Package> {
        self.packages.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&Package> {
        self.packages.get(name)
    }

    pub fn list(&self) -> Vec<Package> {
        self.packages.values().cloned().collect()
    }

    /// Next free UID in the reserved range, ignoring ones already taken by
    /// installed packages.
    pub fn next_uid(&self, start: u32, end: u32) -> Option<u32> {
        (start..=end).find(|uid| !self.packages.values().any(|p| p.uid == *uid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, uid: u32) -> Package {
        Package {
            name: name.to_string(),
            uid,
            data_dir: format!("/var/lib/mosaic/apps/{}", name),
            apk: format!("/tmp/{}.apk", name),
            installed_at: 0,
        }
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        let mut reg = Registry::load(work);
        reg.add(pkg("com.termux", 5000));
        reg.save(work).unwrap();

        let reloaded = Registry::load(work);
        assert_eq!(reloaded.get("com.termux").unwrap().uid, 5000);
    }

    #[test]
    fn corrupt_registry_is_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().to_str().unwrap();
        std::fs::write(format!("{}/registry.json", work), b"{ not json").unwrap();
        assert!(Registry::load(work).list().is_empty());
    }

    #[test]
    fn next_uid_skips_taken_ones() {
        let mut reg = Registry::default();
        reg.add(pkg("a", 5000));
        reg.add(pkg("b", 5002));
        assert_eq!(reg.next_uid(5000, 5003), Some(5001));
        reg.add(pkg("c", 5001));
        assert_eq!(reg.next_uid(5000, 5002), None);
    }
}
