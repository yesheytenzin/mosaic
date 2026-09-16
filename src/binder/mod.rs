// SPDX-License-Identifier: GPL-3.0-or-later

//! Userspace Binder (ADR-0004) and app process launch.
//!
//! Three pieces, kept apart on purpose:
//!
//! - [`table`] is the per-process view: which number names which object.
//! - [`broker`] is the authority: who owns what, and where a transaction goes.
//! - [`wire`] is the transport: frames on a Unix socket, with descriptors.
//!
//! Parcels are not here. The shim writes and reads them, because their formats
//! are Android's and both sides of every AIDL call already agree on them.

pub mod broker;
pub mod table;
pub mod transport;
pub mod wire;

pub use broker::{BinderBroker, ClientId, DeathNotice, Dispatch, HOSTED_NODE_BASE};
pub use table::{HandleTable, Released, Target};
pub use transport::Transport;
pub use wire::{is_binder_prefix, Conn, Frame, Message, MAX_FDS, MAX_FRAME, TF_ONE_WAY};

use crate::args::MosaicArgs;
use crate::broker::registry::Package;
use std::collections::HashMap;

/// The service manager's handle, which is 0 in every process.
pub const SERVICE_MANAGER: ClientId = 0;

/// A process-local reference to a Binder object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    pub node: u64,
    pub owner: u32,
}

impl Handle {
    pub fn new(node: u64, owner: u32) -> Self {
        Self { node, owner }
    }
}

/// Anything reachable over userspace Binder. Transaction payloads are opaque
/// bytes at this layer; the per-interface codecs sit above it.
pub trait BinderObject: Send {
    fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Vec<u8>>;
}

#[derive(Default)]
pub struct ServiceRegistry {
    objects: HashMap<String, Box<dyn BinderObject>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: &str, object: Box<dyn BinderObject>) {
        self.objects.insert(name.to_string(), object);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.objects.contains_key(name)
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.objects.keys().cloned().collect();
        names.sort();
        names
    }

    /// Dispatch a transaction to a registered service.
    pub fn transact(&mut self, name: &str, code: u32, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let object = self
            .objects
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("no such service: {}", name))?;
        object.transact(code, data)
    }
}

/// Start an app process from the runtime bundle.
///
/// Arrives in Phase 2, once a DEX runs on host-native ART. Until then this
/// reports what it would do rather than pretending to succeed.
pub fn launch_app(args: &MosaicArgs, package: &Package, extra: &[String]) -> anyhow::Result<()> {
    let runtime = crate::runtime::require(args)?;
    anyhow::bail!(
        "app process launch is not implemented yet. {} is installed (uid {}, data {}) \
         and the runtime bundle is at {}, but starting an ART process from it is Phase 2{}",
        package.name,
        package.uid,
        package.data_dir,
        runtime,
        if extra.is_empty() {
            String::new()
        } else {
            format!(" (extra args: {})", extra.join(" "))
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl BinderObject for Echo {
        fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Vec<u8>> {
            let mut out = code.to_be_bytes().to_vec();
            out.extend_from_slice(data);
            Ok(out)
        }
    }

    #[test]
    fn registry_dispatches_transactions() {
        let mut registry = ServiceRegistry::new();
        assert!(!registry.contains("echo"));
        registry.register("echo", Box::new(Echo));
        assert!(registry.contains("echo"));
        assert_eq!(registry.names(), vec!["echo".to_string()]);

        let out = registry.transact("echo", 7, b"hi").unwrap();
        assert_eq!(&out[..4], &7u32.to_be_bytes());
        assert_eq!(&out[4..], b"hi");
    }

    #[test]
    fn unknown_service_is_an_error() {
        let mut registry = ServiceRegistry::new();
        assert!(registry.transact("nope", 1, b"").is_err());
    }

    #[test]
    fn handle_carries_owner() {
        let h = Handle::new(3, 5000);
        assert_eq!(h.node, 3);
        assert_eq!(h.owner, 5000);
    }
}
