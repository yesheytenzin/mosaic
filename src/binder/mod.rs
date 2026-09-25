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
pub mod parcel;
pub mod table;
pub mod transport;
pub mod wire;

pub use broker::{BinderBroker, ClientId, DeathNotice, Dispatch, HOSTED_NODE_BASE};
pub use parcel::{Parcel, INTERFACE_TRANSACTION, PING_TRANSACTION};
pub use table::{HandleTable, Released, Target};
pub use transport::Transport;
pub use wire::{is_binder_prefix, Conn, Frame, Message, MAX_FDS, MAX_FRAME, TF_ONE_WAY};

use crate::args::MosaicArgs;
use crate::broker::registry::Package;
use std::collections::HashMap;
use std::os::fd::OwnedFd;

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

/// An object an answer hands back to its caller.
pub enum Handed {
    /// It is already exported, under this node: the caller wants a handle to it.
    Node(u64),
    /// It is new, and belongs to no one until the broker hosts it. A wake lock or a
    /// display token is created by the call that returns it.
    ///
    /// `key` gives it an identity. Two calls that name the same key hand back the
    /// same object rather than two that mean the same thing, which is what a
    /// display token needs: the framework keeps the token and hands it back to ask
    /// about that display, and two tokens for one display would be two answers to
    /// the same question.
    New {
        key: Option<String>,
        object: Box<dyn BinderObject>,
    },
}

impl Handed {
    /// A new object, with the identity it should be remembered by.
    pub fn new(key: impl Into<String>, object: Box<dyn BinderObject>) -> Self {
        Handed::New {
            key: Some(key.into()),
            object,
        }
    }

    /// A new object with no identity: every call makes a fresh one.
    pub fn fresh(object: Box<dyn BinderObject>) -> Self {
        Handed::New { key: None, object }
    }
}

/// A binder object *inside* an answer: where the object word sits in the answer's
/// data, and what to hand the caller.
///
/// The answer names an object, never a handle: a handle is an entry in one
/// process's table, and the object answering does not know which process asked.
/// The transport, which does, writes the caller's handle at that offset before the
/// answer is sent.
pub struct ObjectRef {
    pub offset: u32,
    pub object: Handed,
}

/// What a binder object answered.
#[derive(Default)]
pub struct Answer {
    /// The parcel body the caller reads: an AIDL status word, then the values.
    pub data: Vec<u8>,
    /// Binder objects inside that data, which the caller's side has to register.
    pub objects: Vec<ObjectRef>,
    /// Calls to make after this answer is delivered.
    ///
    /// A service that is handed an object by its caller -- a health callback, a
    /// listener -- has to be able to call it back, and this is how it asks. The
    /// handle is the *caller's*, because the object came from the caller and only
    /// the caller's table knows what it means. The broker routes each one the way
    /// it routes any transaction.
    pub calls: Vec<PendingCall>,
    /// Descriptors inside that data, by offset: the word at each is written with
    /// the *receiving* process's descriptor number, which only the side holding it
    /// knows. A `BitTube` is two of these, which is how a display event connection
    /// hands its caller the channel events arrive on.
    pub fds: Vec<ObjectFd>,
}

/// A call a hosted service wants made once its answer has been delivered.
#[derive(Debug)]
pub struct PendingCall {
    /// A handle in the caller's table: the object the caller passed in.
    pub handle: u32,
    pub code: u32,
    pub data: Vec<u8>,
}

/// A descriptor in an answer: where its word sits, and the descriptor itself.
pub struct ObjectFd {
    pub offset: u32,
    pub fd: OwnedFd,
}

impl From<Vec<u8>> for Answer {
    fn from(data: Vec<u8>) -> Self {
        Self {
            data,
            objects: Vec::new(),
            calls: Vec::new(),
            fds: Vec::new(),
        }
    }
}

impl Answer {
    /// An object handed back at this offset, whose word the caller's side writes.
    pub fn handing(mut self, offset: u32, object: Handed) -> Self {
        self.objects.push(ObjectRef { offset, object });
        self
    }

    /// A descriptor handed back at this offset, for the same reason: the number in
    /// the word has to be one the receiver can open.
    pub fn handing_fd(mut self, offset: u32, fd: OwnedFd) -> Self {
        self.fds.push(ObjectFd { offset, fd });
        self
    }
}

/// Anything reachable over userspace Binder. Transaction payloads are opaque
/// bytes at this layer; the per-interface codecs sit above it.
pub trait BinderObject: Send {
    fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Answer>;

    /// The same call, with the objects the caller passed as arguments.
    ///
    /// Each entry is the *caller's* handle for one of the objects in `data`, in
    /// the order they appear. A service that only reads values keeps the simple
    /// form above and never sees this; one that has to call its caller back --
    /// which is what a callback registration is -- needs the handle, and this is
    /// the only place that can hand it over.
    fn transact_with(
        &mut self,
        code: u32,
        data: &[u8],
        _arguments: &[u32],
    ) -> anyhow::Result<Answer> {
        self.transact(code, data)
    }

    /// The interface this object is, as `IBinder::getInterfaceDescriptor` returns
    /// it. A proxy asks for it over the wire before it will use the object, so an
    /// object that cannot say is an object nothing can call.
    fn descriptor(&self) -> &str {
        ""
    }
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
    pub fn transact(&mut self, name: &str, code: u32, data: &[u8]) -> anyhow::Result<Answer> {
        self.transact_with(name, code, data, &[])
    }

    /// The same call, with the caller's handles for the objects among the
    /// arguments.
    pub fn transact_with(
        &mut self,
        name: &str,
        code: u32,
        data: &[u8],
        arguments: &[u32],
    ) -> anyhow::Result<Answer> {
        let object = self
            .objects
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("no such service: {}", name))?;
        Self::answer_itself(object.as_mut(), code, data, arguments)
    }

    /// One object's answer, including the two codes that belong to the protocol
    /// rather than to any interface. An object the broker hosts without a name --
    /// a wake lock it just made -- is reached this way, not through the registry.
    pub fn answer_itself(
        object: &mut dyn BinderObject,
        code: u32,
        data: &[u8],
        arguments: &[u32],
    ) -> anyhow::Result<Answer> {
        // Two codes belong to the protocol rather than to any interface
        // (`IBinder.h`), and every object answers them: a ping, which is answered
        // by saying nothing, and an interface query, which is answered with the
        // object's descriptor. A proxy asks the second before it will use an
        // object -- answering it with an empty reply makes the proxy report
        // UNKNOWN_TRANSACTION, and the generated code for that failure is a crash.
        if code == PING_TRANSACTION {
            return Ok(Answer::default());
        }
        if code == INTERFACE_TRANSACTION {
            let mut reply = Parcel::new();
            reply.string16(object.descriptor());
            return Ok(reply.into_bytes().into());
        }
        object.transact_with(code, data, arguments)
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
        fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Answer> {
            let mut out = code.to_be_bytes().to_vec();
            out.extend_from_slice(data);
            Ok(out.into())
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
        assert_eq!(&out.data[..4], &7u32.to_be_bytes());
        assert_eq!(&out.data[4..], b"hi");
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
