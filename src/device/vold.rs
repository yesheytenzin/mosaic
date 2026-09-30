// SPDX-License-Identifier: GPL-3.0-or-later

//! `vold`, in userspace: `android.os.IVold`.
//!
//! `StorageManagerService` asks for this one the same way it asks for `storaged`,
//! and the boot waits the same way:
//!
//! ```text
//! StorageManagerService: vold not found; trying again
//! ```
//!
//! On a device `vold` owns volumes: it mounts, unmounts, formats and encrypts the
//! storage Android manages. There is no such storage here -- an app's files live
//! in the bundle and in its own system user's home (ADR-0007) -- so what this
//! answers is that no volume is being managed, which is true rather than a
//! stand-in, and it says so out loud for every call it does not implement.
//!
//! The calls are logged once each: the method numbers are not in the bundle's
//! jars, and reading them out of a boot's log is how the HIDL service manager's
//! codes were learned.

use crate::binder::parcel::{Parcel, Reader};
use crate::binder::{Answer, BinderObject};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The name the framework looks up.
pub const NAME: &str = "vold";

/// The interface's own token, which a proxy writes before it will call the object.
const DESCRIPTOR: &str = "android.os.IVold";

#[derive(Default)]
pub struct Vold;

impl Vold {
    pub fn new() -> Self {
        Self
    }
}

impl BinderObject for Vold {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 64 {
            // The token is the first string of an AIDL request and it names the
            // interface, which is the one thing this stub has to know that the
            // bundle's jars do not carry.
            let token = Reader::new(data).string().unwrap_or_default();
            log::info!("vold: code {code}, {} bytes in, token {token}", data.len());
        }
        // A `void` method gets the empty reply it expects; a method that returns
        // something gets that empty reply too, and the caller reports the read
        // failure rather than a value this side would have had to invent.
        let reply = Parcel::new();
        Ok(reply.into_bytes().into())
    }
}
