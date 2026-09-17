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

    /// The package a person means by `selector`.
    ///
    /// Nobody wants to type `termux.app.v0.119.0.beta.3.apt.android.7.github.debug.x86.64`,
    /// and the name an installed package has is derived from its file anyway. An
    /// exact name wins; failing that a unique prefix; failing that a unique
    /// substring. Anything else is an error that says what it matched, because
    /// "not found" and "several" need different things from the caller.
    pub fn resolve(&self, selector: &str) -> anyhow::Result<&Package> {
        if let Some(package) = self.packages.get(selector) {
            return Ok(package);
        }
        let folded = selector.to_lowercase();
        // Prefix first, then substring: "termux" should not be ambiguous with a
        // package that merely contains it.
        for prefixes_only in [true, false] {
            let mut matches: Vec<&Package> = self
                .packages
                .values()
                .filter(|package| {
                    let name = package.name.to_lowercase();
                    if prefixes_only {
                        name.starts_with(&folded)
                    } else {
                        name.contains(&folded)
                    }
                })
                .collect();
            matches.sort_by(|a, b| a.name.cmp(&b.name));
            match matches.len() {
                0 => continue,
                1 => return Ok(matches[0]),
                _ => {
                    anyhow::bail!(
                        "{} matches {} packages: {}",
                        selector,
                        matches.len(),
                        matches
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        }
        let installed = self.list();
        if installed.is_empty() {
            anyhow::bail!("{} is not installed, and nothing is", selector)
        }
        anyhow::bail!(
            "{} is not installed. Installed: {}",
            selector,
            installed
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
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

    fn registry_with(names: &[&str]) -> Registry {
        let mut registry = Registry::default();
        for (i, name) in names.iter().enumerate() {
            registry.add(Package {
                name: name.to_string(),
                uid: 5000 + i as u32,
                data_dir: format!("/apps/{}", name),
                apk: format!("/apks/{}.apk", name),
                installed_at: 0,
            });
        }
        registry
    }

    /// Nobody types `termux.app.v0.119.0.beta.3...`, and the name is derived from
    /// the file anyway, so a prefix has to find the package.
    #[test]
    fn a_unique_prefix_finds_the_package() {
        let registry = registry_with(&["termux.app.v0.119.0.beta.3", "org.example.other"]);
        assert_eq!(
            registry.resolve("termux").unwrap().name,
            "termux.app.v0.119.0.beta.3"
        );
        // Case does not matter, and an exact name still wins.
        assert_eq!(
            registry.resolve("TERMUX").unwrap().name,
            "termux.app.v0.119.0.beta.3"
        );
        assert_eq!(registry.resolve("org.example.other").unwrap().uid, 5001);
    }

    /// A prefix beats a mere substring, and a substring is the fallback.
    #[test]
    fn prefix_first_then_substring() {
        let registry = registry_with(&["termux.app", "app.termux.helper"]);
        assert_eq!(registry.resolve("termux").unwrap().name, "termux.app");
        assert_eq!(
            registry.resolve("helper").unwrap().name,
            "app.termux.helper"
        );
    }

    /// Several matches need a different answer from none: "which of these?" rather
    /// than "there is nothing".
    #[test]
    fn ambiguity_says_which_it_matched() {
        let registry = registry_with(&["termux.one", "termux.two"]);
        let err = registry.resolve("termux").unwrap_err().to_string();
        assert!(err.contains("matches 2 packages"), "{}", err);
        assert!(
            err.contains("termux.one") && err.contains("termux.two"),
            "{}",
            err
        );
    }

    #[test]
    fn a_miss_names_what_is_installed() {
        let registry = registry_with(&["termux.app"]);
        let err = registry.resolve("nope").unwrap_err().to_string();
        assert!(err.contains("termux.app"), "{}", err);

        let empty = Registry::default();
        let err = empty.resolve("nope").unwrap_err().to_string();
        assert!(err.contains("nothing is"), "{}", err);
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
