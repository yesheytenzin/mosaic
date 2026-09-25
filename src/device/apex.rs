// SPDX-License-Identifier: GPL-3.0-or-later

//! `apexd`, in userspace: the apex service.
//!
//! On a device this is a native daemon that mounts the APEX containers and tells
//! the framework what is installed. The framework asks it over binder for the
//! *active* apexes, and `PackageManagerService` scans the packages inside them:
//! without this service an apex package that is present in the bundle and readable
//! is still unknown to it, which is how the boot came to abort on
//! `Required services extension package is missing` while that package was sitting
//! in the bundle the whole time.
//!
//! There is no mounting here and there is nothing to stage: this host runs the
//! image's files directly rather than containerising them. What it has is the list
//! of apexes the runtime bundle carries, which it answers with.
//!
//! Everything outside the boot path -- staging sessions, rollbacks, snapshots,
//! compressed-apex space -- is refused by name, the way `installd` refuses what it
//! does not do. A silent success there would be a lie about an update that never
//! happened.

use crate::binder::parcel::Parcel;
use crate::binder::{Answer, BinderObject};
use anyhow::Result;

/// The name the framework looks up (`ApexService::instantiate`).
pub const NAME: &str = "apexservice";

/// `android.apex.IApexService`, in declaration order.
mod code {
    pub const GET_SESSIONS: u32 = 4;
    pub const GET_ACTIVE_PACKAGES: u32 = 7;
    pub const GET_ALL_PACKAGES: u32 = 8;
    pub const ABORT_STAGED_SESSION: u32 = 9;
    pub const REVERT_ACTIVE_SESSIONS: u32 = 10;
    pub const GET_ACTIVE_PACKAGE: u32 = 17;
    pub const MARK_BOOT_COMPLETED: u32 = 23;
}

/// `Status::EX_UNSUPPORTED_OPERATION`, as an AIDL exception code.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// One apex the bundle carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Apex {
    pub module_name: String,
    /// The path the framework reads the module from. The path shim resolves
    /// `/system/apex/...` into the bundle's `apex/` directory.
    pub module_path: String,
    pub version_code: i64,
    pub version_name: String,
}

/// The apexes in the bundle, from its `apex/` directory.
///
/// A directory per module, which is how this image ships them -- flattened, not as
/// `.apex` containers -- so there is nothing to mount and nothing to decompress.
pub fn apexes(root: &str) -> Vec<Apex> {
    let mut found = Vec::new();
    let dir = std::path::Path::new(root).join("apex");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return found;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    // The version the framework sees for every module. A device reads it out of
    // each apex's manifest; these are the bundle's build, and `ApexManager` uses
    // the pair for ordering updates it is not doing here.
    let (version_code, version_name) = build_version(root);
    for name in names {
        found.push(Apex {
            module_path: format!("/system/apex/{}", name),
            module_name: name,
            version_code,
            version_name: version_name.clone(),
        });
    }
    found
}

/// `ro.build.id` and a version code derived from the SDK level, from the bundle's
/// own build.prop, so a rebuilt bundle answers with its own numbers.
fn build_version(root: &str) -> (i64, String) {
    let mut id = String::from("unknown");
    let mut sdk = 33i64;
    if let Ok(text) =
        std::fs::read_to_string(std::path::Path::new(root).join("etc").join("build.prop"))
    {
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("ro.build.id=") {
                id = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("ro.build.version.sdk=") {
                sdk = value.trim().parse().unwrap_or(33);
            }
        }
    }
    // A device's version code is the SDK level in the high half and a build
    // counter below it; nothing here compares it, so the shape is what matters.
    (sdk << 24, id)
}

pub struct ApexService {
    apexes: Vec<Apex>,
}

impl ApexService {
    pub fn new(root: &str) -> Self {
        let apexes = apexes(root);
        for apex in &apexes {
            log::info!("apexservice: {} at {}", apex.module_name, apex.module_path);
        }
        if apexes.is_empty() {
            log::info!("apexservice: no apexes in {}/apex", root);
        }
        Self { apexes }
    }

    /// One `android.apex.ApexInfo`, as its AIDL declaration writes it: three
    /// strings, a long, a string, then four booleans.
    fn apex_info(&self, apex: &Apex, out: &mut Parcel) {
        out.string16(&apex.module_name);
        out.string16(&apex.module_path);
        out.string16(&apex.module_path); // preinstalledModulePath: the same here
        out.i64(apex.version_code);
        out.string16(&apex.version_name);
        out.boolean(true); // isFactory
        out.boolean(true); // isActive
        out.boolean(false); // hasClassPathJars: populated only for staged apexes
        out.boolean(false); // activeApexChanged: only ever set during a boot
    }

    fn packages(&self, out: &mut Parcel) {
        out.i32(self.apexes.len() as i32);
        for apex in &self.apexes {
            self.apex_info(apex, out);
        }
    }

    fn refuse(&self, code: u32) -> Answer {
        let mut reply = Parcel::new();
        reply.i32(EX_UNSUPPORTED_OPERATION);
        reply.string16(&format!("IApexService method {} is not implemented", code));
        reply.i32(0); // the remote stack trace header the reader expects
        reply.into_bytes().into()
    }
}

impl BinderObject for ApexService {
    fn descriptor(&self) -> &str {
        "android.apex.IApexService"
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        match code {
            // What `ApexManager.getActiveApexInfos` asks for, and the reason this
            // service exists: the package manager scans what it names.
            code::GET_ACTIVE_PACKAGES | code::GET_ALL_PACKAGES => {
                let mut reply = Parcel::new();
                self.packages(&mut reply);
                Ok(reply.into_bytes().into())
            }
            // `getActivePackage(String)`: one apex by module name, or null.
            code::GET_ACTIVE_PACKAGE => {
                let mut reader = crate::binder::parcel::Reader::new(data);
                let name = reader.string().unwrap_or_default();
                let mut reply = Parcel::new();
                match self.apexes.iter().find(|apex| apex.module_name == name) {
                    Some(apex) => {
                        reply.boolean(true);
                        self.apex_info(apex, &mut reply);
                    }
                    None => reply.boolean(false),
                }
                Ok(reply.into_bytes().into())
            }
            // The framework telling the service the boot is over. Nothing here
            // waits on a boot, and nothing needs to be told.
            code::MARK_BOOT_COMPLETED => {
                let reply = Parcel::new();
                Ok(reply.into_bytes().into())
            }
            // Sessions, rollbacks and snapshots are an update mechanism this host
            // does not have. An empty session list is the truth of that: no update
            // has ever been staged.
            code::GET_SESSIONS => {
                let mut reply = Parcel::new();
                reply.i32(0);
                Ok(reply.into_bytes().into())
            }
            code::ABORT_STAGED_SESSION | code::REVERT_ACTIVE_SESSIONS => {
                let reply = Parcel::new();
                Ok(reply.into_bytes().into())
            }
            other => Ok(self.refuse(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::parcel::Reader;

    fn service() -> ApexService {
        ApexService {
            apexes: vec![
                Apex {
                    module_name: "com.android.extservices".to_string(),
                    module_path: "/system/apex/com.android.extservices".to_string(),
                    version_code: 0x2100_0000,
                    version_name: "TQ3A.230901.001".to_string(),
                },
                Apex {
                    module_name: "com.android.i18n".to_string(),
                    module_path: "/system/apex/com.android.i18n".to_string(),
                    version_code: 0x2100_0000,
                    version_name: "TQ3A.230901.001".to_string(),
                },
            ],
        }
    }

    #[test]
    fn the_active_packages_are_the_apexes_it_carries() {
        let mut service = service();
        let reply = service
            .transact(code::GET_ACTIVE_PACKAGES, &[])
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 2);
        // The first apex, field by field, in the order ApexInfo.aidl declares.
        assert_eq!(reader.string().as_deref(), Some("com.android.extservices"));
        assert_eq!(
            reader.string().as_deref(),
            Some("/system/apex/com.android.extservices")
        );
        assert_eq!(
            reader.string().as_deref(),
            Some("/system/apex/com.android.extservices")
        );
        assert_eq!(reader.i64(), 0x2100_0000);
        assert_eq!(reader.string().as_deref(), Some("TQ3A.230901.001"));
        assert_eq!(reader.i32(), 1); // isFactory
        assert_eq!(reader.i32(), 1); // isActive
        assert_eq!(reader.i32(), 0); // hasClassPathJars
        assert_eq!(reader.i32(), 0); // activeApexChanged
    }

    #[test]
    fn one_apex_can_be_asked_for_by_name() {
        let mut service = service();
        let mut args = Parcel::new();
        args.string16("com.android.i18n");
        let reply = service
            .transact(code::GET_ACTIVE_PACKAGE, &args.into_bytes())
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 1); // present
        assert_eq!(reader.string().as_deref(), Some("com.android.i18n"));

        // A module that is not there is absent, not an error: the caller is asking
        // a question and "no" is an answer.
        let mut args = Parcel::new();
        args.string16("com.android.nothing");
        let reply = service
            .transact(code::GET_ACTIVE_PACKAGE, &args.into_bytes())
            .unwrap()
            .data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), 0);
        assert_eq!(reader.i32(), 0);
    }

    #[test]
    fn the_update_machinery_is_refused_by_name() {
        let mut service = service();
        let reply = service.transact(1, &[]).unwrap().data;
        let mut reader = Reader::new(&reply);
        assert_eq!(reader.i32(), EX_UNSUPPORTED_OPERATION);
        assert!(reader
            .string()
            .unwrap_or_default()
            .contains("IApexService method 1"));
    }
}
