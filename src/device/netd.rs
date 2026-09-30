// SPDX-License-Identifier: GPL-3.0-or-later

//! `netd`, in userspace: `android.system.net.netd.INetd`.
//!
//! This is where the boot stops now. `SystemServer` reaches
//! `StartNetworkManagementService`, which asks for the daemon and waits when it
//! cannot:
//!
//! ```text
//! NetdService: WARNING: returning null INetd instance.
//! ```
//!
//! and with the name unhosted the stage never completes. On a device `netd`
//! owns the network: interface configuration, routing, firewalling and the
//! per-uid accounting the framework reads back. Mosaic runs apps as ordinary host
//! processes on the host's own network (ADR-0001), so there is no interface for
//! this side to configure -- the truthful answer to "create a network" here is
//! that the host's network is the network, and this service is where that answer
//! is given, method by method, logging each one so the method numbers can be read
//! out of a boot's log rather than guessed.

use crate::binder::parcel::{Parcel, Reader};
use crate::binder::{Answer, BinderObject};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The name the framework looks up.
pub const NAME: &str = "netd";

/// The interface's own token, which a proxy writes before it will call the object.
const DESCRIPTOR: &str = "android.system.net.netd.INetd";

#[derive(Default)]
pub struct Netd;

impl Netd {
    pub fn new() -> Self {
        Self
    }
}

impl BinderObject for Netd {
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
            log::info!("netd: code {code}, {} bytes in, token {token}", data.len());
        }
        // Answered empty until the log says which codes the boot calls and what
        // each one returns: a `void` method reads the empty parcel as success, and
        // one that returns a value reports the read failure instead of a number
        // invented here.
        let reply = Parcel::new();
        Ok(reply.into_bytes().into())
    }
}
