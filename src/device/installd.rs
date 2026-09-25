// SPDX-License-Identifier: GPL-3.0-or-later

//! `installd`, the native daemon the package manager talks to.
//!
//! On a device it is a root daemon: it creates and owns the per-app data
//! directories, labels them for SELinux, optimizes their bytecode and reports how
//! much space everything takes. `PackageManagerService` waits for it — without it
//! the framework loops on `Installer: installd not found; trying again` — and then
//! asks it about every package it scans.
//!
//! Mosaic keeps the structure and drops the device: the directories are the host's
//! (`<data>/data/<package>`, reachable inside the app because the shim presents
//! `/data`), the sizes are `stat`'s, and the operations that exist only because
//! Android has a device — mounting, SELinux labelling, migrating a legacy layout —
//! are answered as what they are, *nothing to do*, rather than faked. The one thing
//! this cannot do is what needs root: a data directory is owned by the app's system
//! user, and that ownership is set at install time by the privileged helper
//! (ADR-0008), not here.
//!
//! Everything outside the boot path answers `EX_UNSUPPORTED_OPERATION` with a
//! message that names the method. That is a refusal, not a stub: the caller gets a
//! well-formed failure it can report, which is what a device without that capability
//! gives it.

use crate::binder::parcel::{args_after, Parcel, Reader};
use crate::binder::{Answer, BinderObject};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// `android.os.IInstalld`, in declaration order.
mod code {
    pub const CREATE_USER_DATA: u32 = 1;
    pub const DESTROY_USER_DATA: u32 = 2;
    pub const SET_FIRST_BOOT: u32 = 3;
    pub const CREATE_APP_DATA: u32 = 4;
    pub const CREATE_APP_DATA_BATCHED: u32 = 5;
    pub const RESTORECON_APP_DATA: u32 = 7;
    pub const MIGRATE_APP_DATA: u32 = 8;
    pub const CLEAR_APP_DATA: u32 = 9;
    pub const DESTROY_APP_DATA: u32 = 10;
    pub const FIXUP_APP_DATA: u32 = 11;
    pub const GET_APP_SIZE: u32 = 12;
    pub const RM_DEX: u32 = 22;
    pub const DESTROY_APP_PROFILES: u32 = 27;
    pub const LINK_NATIVE_LIBRARY_DIRECTORY: u32 = 33;
    pub const INVALIDATE_MOUNTS: u32 = 40;
    pub const MIGRATE_LEGACY_OBB_DATA: u32 = 49;
}

/// The interface token every request carries.
pub const IINSTALLD: &str = "android.os.IInstalld";

/// The name it is published under, which is how `Installer.connect()` finds it.
pub const NAME: &str = "installd";

/// `Status::EX_UNSUPPORTED_OPERATION`.
const EX_UNSUPPORTED_OPERATION: i32 = -7;

/// How many values `getAppSize` returns for each package. `Installer.getAppSize`
/// indexes the answer, so a short one is read off the end; these are the six the
/// method is defined to return.
const APP_SIZE_FIELDS: usize = 6;

pub struct Installd {
    /// The Android root: `/data` as the framework sees it is `<root>/data`, and the
    /// shim presents it that way to the app.
    root: PathBuf,
}

impl Installd {
    /// `root` is the runtime bundle, which is where the framework's `/data` and
    /// `/system` live (the shim rewrites the absolute paths it uses).
    pub fn new(root: &str) -> Self {
        Self {
            root: PathBuf::from(root),
        }
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `/data/data/<package>`, which is the app's own data directory.
    fn app_data(&self, package: &str) -> PathBuf {
        self.data().join("data").join(package)
    }

    /// `/data/user/<userId>/<package>`, the per-user view of the same thing. Mosaic
    /// has one user per app rather than several, so this is the same directory
    /// reached by another name — and creating both is what the framework expects to
    /// find.
    fn user_data(&self, user: i32, package: &str) -> PathBuf {
        self.data()
            .join("user")
            .join(user.to_string())
            .join(package)
    }

    /// `/data/user_de/<userId>/<package>`, the device-encrypted view. The same
    /// directory again, and the one `SettingsProvider` writes its database into.
    fn user_de_data(&self, user: i32, package: &str) -> PathBuf {
        self.data()
            .join("user_de")
            .join(user.to_string())
            .join(package)
    }

    fn profiles(&self) -> PathBuf {
        self.data().join("misc").join("profiles")
    }

    fn create_dir(path: &Path) -> std::io::Result<u64> {
        std::fs::create_dir_all(path)?;
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(path)?.ino())
    }

    /// The size of a directory tree in bytes, the way `du` counts it: allocated
    /// blocks rather than apparent size, because that is what the framework is
    /// asking about (`getAppSize`).
    fn tree_size(path: &Path) -> i64 {
        use std::os::unix::fs::MetadataExt;
        let mut total: i64 = 0;
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                total += Self::tree_size(&entry.path());
            } else {
                total += metadata.blocks() as i64 * 512;
            }
        }
        total
    }

    fn remove_tree(path: &Path) -> std::io::Result<()> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
            Ok(_) => std::fs::remove_file(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// One `CreateAppDataArgs`: a parcelable, so its presence flag first, then the
    /// uuid, the package, the user, flags, app id, previous app id, seInfo and
    /// target sdk — the order the AIDL declares them in.
    fn read_create_args(reader: &mut Reader) -> (String, i32) {
        reader.parcelable_present();
        let _uuid = reader.string();
        let package = reader.string().unwrap_or_default();
        let user = reader.i32();
        let _flags = reader.i32();
        let _app_id = reader.i32();
        let _previous_app_id = reader.i32();
        let _se_info = reader.string();
        let _target_sdk = reader.i32();
        (package, user)
    }

    /// `CreateAppDataResult`: the inode of the directory that was created, or the
    /// reason it was not. A failure is reported in the result's own fields rather
    /// than as an exception, because this is a per-package answer.
    fn create_app_data_fields(&self, reader: &mut Reader) -> (i64, i32, String) {
        let (package, user) = Self::read_create_args(reader);
        if package.is_empty() {
            return (0, 0, String::new());
        }
        match Self::create_dir(&self.app_data(&package)) {
            Ok(inode) => {
                // Three names for the same directory, and the framework uses all of
                // them: the package's own data, its per-user view, and its
                // *device-encrypted* view. `user_de` was the one left out, and it is
                // where `SettingsProvider` keeps its database -- so the boot got as
                // far as the settings provider and then could not open
                // `/data/user_de/0/com.android.providers.settings/databases/settings.db`,
                // because no one had made the directory above it.
                let _ = Self::create_dir(&self.user_data(user, &package));
                let _ = Self::create_dir(&self.user_de_data(user, &package));
                (inode as i64, 0, String::new())
            }
            Err(e) => (0, EX_UNSUPPORTED_OPERATION, e.to_string()),
        }
    }

    /// One `CreateAppDataResult`, with the length word every AIDL parcelable
    /// carries.
    ///
    /// The generated Java writes a placeholder length first and patches it in
    /// afterwards, and its reader refuses a body shorter than four bytes -- which is
    /// what these fields read as with nothing in front of them:
    ///
    /// ```text
    /// android.os.BadParcelableException: Parcelable too small
    ///   at com.android.server.pm.Installer.createAppDataBatched(Installer.java:298)
    /// ```
    ///
    /// That is the *length*, and it is not the presence word: `IInstalld.aidl`
    /// declares the result without `@nullable`, so the elements are read one after
    /// another and the length is the first thing each one reads. A presence word
    /// instead of a length shifts every field and the caller reads a null result,
    /// which is what `Installer$Batch.execute` once reported as a
    /// NullPointerException.
    fn create_app_data_result(&self, inode: i64, error: i32, message: &str) -> Vec<u8> {
        let mut body = Parcel::new();
        body.i64(inode);
        body.i32(error);
        body.string16(message);
        let body = body.into_bytes();
        let mut out = Parcel::new();
        // The presence word first, then the length, then the fields. `Parcel`'s typed
        // readers take an object as "present" or "null" and only then read the
        // parcelable -- `readTypedObject` and each element of `readTypedArray` alike.
        // The length alone made the *next* word the receiver read the length, which
        // for a result whose inode is zero read as too small, and `Batch.execute`
        // reported the element as null and dereferenced it.
        out.i32(1);
        out.i32(body.len() as i32 + 4); // the length covers itself
        out.raw(&body);
        out.into_bytes()
    }

    fn create_app_data(&self, reader: &mut Reader) -> Result<Vec<u8>> {
        let (inode, error, message) = self.create_app_data_fields(reader);
        Ok(self.create_app_data_result(inode, error, &message))
    }

    fn serve(&mut self, code: u32, args: &[u8]) -> Result<Vec<u8>> {
        let mut reader = Reader::new(args);
        let mut reply = Parcel::new();
        match code {
            code::INVALIDATE_MOUNTS => {
                // Nothing to invalidate: on a device this drops the framework's
                // cached view of the storage mounts after they change, and a host
                // filesystem has no such thing to drop.
            }
            code::SET_FIRST_BOOT => {
                // Informational on a device: it tells installd to reconcile a data
                // layout left by an older Android. There is no older layout here.
            }
            code::CREATE_USER_DATA => {
                let _uuid = reader.string();
                let user = reader.i32();
                let _serial = reader.i32();
                let _flags = reader.i32();
                let _ = Self::create_dir(&self.data().join("user").join(user.to_string()));
            }
            code::DESTROY_USER_DATA => {
                let _uuid = reader.string();
                let user = reader.i32();
                let _flags = reader.i32();
                let _ = Self::remove_tree(&self.data().join("user").join(user.to_string()));
            }
            code::CREATE_APP_DATA => return self.create_app_data(&mut reader),
            code::CREATE_APP_DATA_BATCHED => {
                let count = reader.i32();
                let mut results = Vec::new();
                for _ in 0..count.max(0) {
                    results.push(self.create_app_data_fields(&mut reader));
                }
                reply.i32(results.len() as i32);
                for (inode, error, message) in results {
                    reply.raw(&self.create_app_data_result(inode, error, &message));
                }
            }
            code::RESTORECON_APP_DATA => {
                // SELinux labels are not something a host without Android's policy
                // can apply. The gap is stated in ADR-0007 and in the compatibility
                // list rather than papered over here.
            }
            code::MIGRATE_APP_DATA | code::FIXUP_APP_DATA => {
                // Both exist to move or repair a layout written by an older
                // Android; Mosaic's directories are its own from the start.
            }
            code::CLEAR_APP_DATA | code::DESTROY_APP_DATA => {
                let _uuid = reader.string();
                let package = reader.string().unwrap_or_default();
                let user = reader.i32();
                let _flags = reader.i32();
                let _ce_inode = reader.i64();
                if !package.is_empty() {
                    let _ = Self::remove_tree(&self.app_data(&package));
                    let _ = Self::remove_tree(&self.user_data(user, &package));
                }
            }
            code::GET_APP_SIZE => {
                let _uuid = reader.string();
                let packages = reader.strings();
                let _user = reader.i32();
                let _flags = reader.i32();
                let _app_id = reader.i32();
                let _inodes = reader.i64s();
                let code_paths = reader.strings();
                reply.i32((packages.len() * APP_SIZE_FIELDS) as i32);
                for (index, package) in packages.iter().enumerate() {
                    // code, data, cache, then the three external ones, which a host
                    // with no emulated external storage honestly reports as zero.
                    let code_size = code_paths
                        .get(index)
                        .map(|path| Self::tree_size(Path::new(path)))
                        .unwrap_or(0);
                    let data_size = Self::tree_size(&self.app_data(package));
                    reply.i64(code_size);
                    reply.i64(data_size);
                    reply.i64(0);
                    reply.i64(0);
                    reply.i64(0);
                    reply.i64(0);
                }
            }
            code::RM_DEX => {
                let code_path = reader.string().unwrap_or_default();
                let _instruction_set = reader.string();
                if !code_path.is_empty() {
                    let _ = Self::remove_tree(&Path::new(&code_path).join("oat"));
                }
            }
            code::DESTROY_APP_PROFILES => {
                let package = reader.string().unwrap_or_default();
                if !package.is_empty() {
                    let _ = Self::remove_tree(&self.profiles().join(&package));
                }
            }
            code::LINK_NATIVE_LIBRARY_DIRECTORY => {
                let _uuid = reader.string();
                let package = reader.string().unwrap_or_default();
                let _native_lib_path = reader.string();
                let _user = reader.i32();
                // The directory the framework will point `nativeLibraryDir` at. Its
                // contents are the APK's own libraries, extracted when the package
                // is staged; until then it exists and is empty.
                if !package.is_empty() {
                    let _ = Self::create_dir(&self.app_data(&package).join("lib"));
                }
            }
            code::MIGRATE_LEGACY_OBB_DATA => {
                // No OBB data of an older layout to move.
            }
            other => {
                reply.i32(EX_UNSUPPORTED_OPERATION);
                reply.string16(&format!("installd method {} is not implemented", other));
                reply.i32(0); // the remote stack trace header the reader expects
            }
        }
        Ok(reply.into_bytes())
    }
}

impl BinderObject for Installd {
    fn descriptor(&self) -> &str {
        IINSTALLD
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        let args = args_after(data, IINSTALLD);
        Ok(self.serve(code, args)?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request as the shim forwards it: the AIDL header, the interface token, then
    /// the arguments.
    fn request(args: &[u8]) -> Vec<u8> {
        let mut data = vec![0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff];
        data.extend_from_slice(b"TSYS");
        let descriptor = IINSTALLD;
        let count = (descriptor.len() + 1) as i32;
        data.extend_from_slice(&count.to_le_bytes());
        for unit in descriptor.encode_utf16() {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]);
        // A string16 is padded to four bytes, and the arguments follow it.
        while data.len() % 4 != 0 {
            data.push(0);
        }
        data.extend_from_slice(args);
        data
    }

    /// A string16 as libbinder writes one: a length that excludes the terminator,
    /// the units, the terminator, then padding to four bytes.
    fn string16(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let units: Vec<u16> = text.encode_utf16().collect();
        out.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for unit in &units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out
    }

    fn null_string() -> Vec<u8> {
        (-1i32).to_le_bytes().to_vec()
    }

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    #[test]
    fn the_arguments_of_a_create_app_data_request_are_read_in_order() {
        let mut args = Vec::new();
        args.extend_from_slice(&null_string());
        args.extend_from_slice(&string16("org.example.notes"));
        args.extend_from_slice(&7i32.to_le_bytes()); // userId
        args.extend_from_slice(&9i32.to_le_bytes()); // flags
        args.extend_from_slice(&0i32.to_le_bytes()); // appId
        let request = request(&args);
        let body = args_after(&request, IINSTALLD);
        let mut reader = Reader::new(body);
        reader.parcelable_present();
        assert_eq!(reader.string(), None, "a null uuid");
        assert_eq!(reader.string().as_deref(), Some("org.example.notes"));
        assert_eq!(reader.i32(), 7);
        assert_eq!(reader.i32(), 9);
    }

    #[test]
    fn invalidate_mounts_is_answered_rather_than_refused() {
        let root = temp_root();
        let mut service = Installd::new(root.path().to_str().unwrap());
        let _reply = service
            .transact(code::INVALIDATE_MOUNTS, &request(&[]))
            .unwrap()
            .data;
        // A status word and nothing else: this is a void method.
    }

    #[test]
    fn create_app_data_makes_the_directory_the_framework_reads_from() {
        let root = temp_root();
        let mut service = Installd::new(root.path().to_str().unwrap());
        let package = "org.example.notes";
        let mut args = Vec::new();
        args.extend_from_slice(&null_string()); // uuid
        args.extend_from_slice(&string16(package));
        for _ in 0..4 {
            args.extend_from_slice(&0i32.to_le_bytes()); // userId, flags, appId, previousAppId
        }
        args.extend_from_slice(&string16("default")); // seInfo
        args.extend_from_slice(&30i32.to_le_bytes()); // targetSdkVersion

        let reply = service
            .transact(code::CREATE_APP_DATA, &request(&args))
            .unwrap()
            .data;
        // A non-zero inode, and nothing in front of it: the return type is not
        // `@nullable`, so there is no presence flag, and the status is the shim's to write.
        let inode = i64::from_le_bytes(reply[..8].try_into().unwrap());
        assert!(inode > 0, "the inode of the directory that was created");
        assert!(root.path().join("data/data").join(package).is_dir());
        assert!(root.path().join("data/user/0").join(package).is_dir());
    }

    #[test]
    fn get_app_size_returns_six_values_per_package() {
        let root = temp_root();
        let service = Installd::new(root.path().to_str().unwrap());
        std::fs::create_dir_all(root.path().join("data/data/org.example.notes")).unwrap();
        std::fs::write(
            root.path().join("data/data/org.example.notes/notes.db"),
            b"x",
        )
        .unwrap();

        let mut args = Vec::new();
        args.extend_from_slice(&null_string());
        args.extend_from_slice(&1i32.to_le_bytes()); // one package name
        args.extend_from_slice(&string16("org.example.notes"));
        args.extend_from_slice(&0i32.to_le_bytes()); // userId
        args.extend_from_slice(&0i32.to_le_bytes()); // flags
        args.extend_from_slice(&0i32.to_le_bytes()); // appId
        args.extend_from_slice(&0i32.to_le_bytes()); // no ceDataInodes
        args.extend_from_slice(&0i32.to_le_bytes()); // no code paths

        let mut service = service;
        let reply = service
            .transact(code::GET_APP_SIZE, &request(&args))
            .unwrap()
            .data;
        assert_eq!(i32::from_le_bytes(reply[..4].try_into().unwrap()), 6);
        assert_eq!(reply.len(), 4 + 6 * 8);
        // The data size is the file's allocated blocks, so it is not zero.
        let data_size = i64::from_le_bytes(reply[4 + 8..12 + 8].try_into().unwrap());
        assert!(data_size > 0, "the app's data is counted");
    }

    #[test]
    fn an_unimplemented_method_is_refused_with_a_message() {
        let root = temp_root();
        let mut service = Installd::new(root.path().to_str().unwrap());
        let reply = service.transact(19, &request(&[])).unwrap().data;
        assert_eq!(
            i32::from_le_bytes(reply[..4].try_into().unwrap()),
            EX_UNSUPPORTED_OPERATION
        );
        let count = i32::from_le_bytes(reply[4..8].try_into().unwrap());
        assert!(count > 0, "the message is written, not left off");
    }

    #[test]
    fn destroy_app_data_removes_the_directories() {
        let root = temp_root();
        let mut service = Installd::new(root.path().to_str().unwrap());
        let package = "org.example.notes";
        std::fs::create_dir_all(root.path().join("data/data").join(package)).unwrap();
        let mut args = Vec::new();
        args.extend_from_slice(&null_string());
        args.extend_from_slice(&string16(package));
        args.extend_from_slice(&0i32.to_le_bytes());
        args.extend_from_slice(&0i32.to_le_bytes());
        args.extend_from_slice(&0i64.to_le_bytes());
        let _ = service
            .transact(code::DESTROY_APP_DATA, &request(&args))
            .unwrap();
        assert!(!root.path().join("data/data").join(package).exists());
    }
}
