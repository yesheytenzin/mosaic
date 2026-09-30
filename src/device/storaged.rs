// SPDX-License-Identifier: GPL-3.0-or-later

//! `storaged`, in userspace: `android.os.IStoraged`.
//!
//! The framework's `StorageManagerService` is written to refuse to start when the
//! name does not resolve -- it announces it once a second, and the boot waits
//! where it stands:
//!
//! ```text
//! StorageManagerService: storaged not found; trying again
//! ```
//!
//! On a device this daemon is a native binary that reads the block layer's
//! statistics and reports them. A host's storage is not a device's, and nothing
//! here can or should report it as one; what this answers is that the machinery
//! is up, which is what the caller asks. The calls themselves are logged, because
//! the method numbers are not in the bundle's jars and the log is where they come
//! from -- the same loop the HIDL service manager's codes were learned with.

use crate::binder::parcel::{Parcel, Reader};
use crate::binder::{Answer, BinderObject};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The name the framework looks up.
pub const NAME: &str = "storaged";

/// The interface's own token, which a proxy writes before it will call the object.
const DESCRIPTOR: &str = "android.os.IStoraged";

#[derive(Default)]
pub struct Storaged;

impl Storaged {
    pub fn new() -> Self {
        Self
    }
}

impl BinderObject for Storaged {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        // Every call is logged once, with its code and the argument width, so that
        // reading one boot's log is enough to know which methods this has to answer
        // and with what shape.
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 64 {
            // The token is the first string of an AIDL request and it names the
            // interface, which is the one thing this stub has to know that the
            // bundle's jars do not carry.
            let token = Reader::new(data).string().unwrap_or_default();
            log::info!(
                "storaged: code {code}, {} bytes in, token {token}",
                data.len()
            );
        }
        // An AIDL method that returns a value gets an empty reply, which the reader
        // reports as the failure it is; a `void` one gets exactly what it expects.
        // Which of the two each code is, is what the log above is for.
        let reply = Parcel::new();
        Ok(reply.into_bytes().into())
    }
}
