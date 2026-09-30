// SPDX-License-Identifier: GPL-3.0-or-later

//! `dnsresolver`, in userspace.
//!
//! `ConnectivityService`'s constructor asks for this one through the module it was
//! loaded from, and a lookup that finds nothing leaves it holding null:
//!
//! ```text
//! method 'android.os.IBinder android.net.DnsResolverServiceManager.getService()'
//! on a null object reference
//!     at com.android.server.ConnectivityService.getDnsResolver(ConnectivityService.java:1091)
//!     at com.android.server.ConnectivityService.<init>(ConnectivityService.java:1795)
//! ```
//!
//! On a device `dnsresolver` is a native daemon the resolver talks to for every
//! lookup, and it is the reason an app's DNS goes through the platform rather than
//! the libc directly. Apps here are ordinary host processes on the host's network
//! (ADR-0001): their DNS is the host's, resolved by the host's own resolver in the
//! ordinary way, and this service is where that answer is given rather than a
//! second resolver being invented.
//!
//! What it answers, it answers empty until the *log* says which methods the boot
//! calls and with what shapes -- the method numbers are not in the bundle's jars
//! and reading them out of a run is how the HIDL service manager's codes and the
//! other daemons' were learned.

use crate::binder::parcel::{Parcel, Reader};
use crate::binder::{Answer, BinderObject};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The name the framework looks up.
pub const NAME: &str = "dnsresolver";

/// The interface's own token, which a proxy writes before it will call the object.
const DESCRIPTOR: &str = "android.net.IDnsResolver";

#[derive(Default)]
pub struct DnsResolver;

impl DnsResolver {
    pub fn new() -> Self {
        Self
    }
}

impl BinderObject for DnsResolver {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&mut self, code: u32, data: &[u8]) -> Result<Answer> {
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 64 {
            let token = Reader::new(data).string().unwrap_or_default();
            log::info!(
                "dnsresolver: code {code}, {} bytes in, token {token}",
                data.len()
            );
        }
        let reply = Parcel::new();
        Ok(reply.into_bytes().into())
    }
}
