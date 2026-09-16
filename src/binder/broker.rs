// SPDX-License-Identifier: GPL-3.0-or-later

//! The Binder authority: who owns what, who may reach it, and what happens
//! when a process goes away (A2, A3, A5).
//!
//! The broker hosts this. It is the userspace stand-in for the driver's
//! bookkeeping: one handle table per connected process, one record per node,
//! and the routing rule between them. Nothing here knows about parcels -- the
//! shim keeps that, because the parcel formats are Android's.

use crate::binder::table::{HandleTable, Released, Target};
use crate::binder::{BinderObject, ServiceRegistry, SERVICE_MANAGER};
use std::collections::HashMap;

/// A connected process, as the broker numbers them. Zero is the broker itself,
/// which is where services the Rust side provides live.
pub type ClientId = u32;

/// The broker's own services get node ids that a process-allocated id cannot
/// collide with. A client's node is `pid << 32 | counter`, so this is above
/// every one of those.
pub const HOSTED_NODE_BASE: u64 = 1 << 40;

/// What the broker did with a transaction.
#[derive(Debug)]
pub enum Dispatch {
    /// The broker answered it, from a service it hosts.
    Reply { status: i32, data: Vec<u8> },
    /// The caller owns the target. It runs it itself; the broker only had to
    /// say so.
    Local { node: u64 },
    /// The target belongs to another process. The transport sends it there and
    /// relays the answer.
    Forward {
        owner: ClientId,
        node: u64,
        code: u32,
        flags: u32,
        data: Vec<u8>,
    },
}

/// A client that should be told a handle died.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathNotice {
    pub client: ClientId,
    pub handle: u32,
}

struct Node {
    owner: ClientId,
    name: Option<String>,
}

struct Client {
    pid: u32,
    table: HandleTable,
    /// Handles this client asked to be told about, and who owns each.
    links: Vec<(u32, ClientId)>,
}

#[derive(Default)]
pub struct BinderBroker {
    hosted: ServiceRegistry,
    nodes: HashMap<u64, Node>,
    clients: HashMap<ClientId, Client>,
    next_client: ClientId,
    next_hosted_node: u64,
}

impl BinderBroker {
    pub fn new() -> Self {
        Self {
            hosted: ServiceRegistry::new(),
            nodes: HashMap::new(),
            clients: HashMap::new(),
            next_client: 1,
            next_hosted_node: HOSTED_NODE_BASE,
        }
    }

    /// Host an object in the broker, so every process can reach it. Returns the
    /// node it is exported as, which is what a name resolves to.
    pub fn host(&mut self, name: &str, object: Box<dyn BinderObject>) -> u64 {
        let node = self.next_hosted_node;
        self.next_hosted_node += 1;
        self.hosted.register(name, object);
        self.nodes.insert(
            node,
            Node {
                owner: SERVICE_MANAGER,
                name: Some(name.to_string()),
            },
        );
        node
    }

    /// A new connection. The pid is the peer's, from the kernel.
    pub fn connect(&mut self, pid: u32) -> ClientId {
        let id = self.next_client;
        self.next_client += 1;
        self.clients.insert(
            id,
            Client {
                pid,
                table: HandleTable::new(),
                links: Vec::new(),
            },
        );
        log::debug!("Binder client {} connected (pid {})", id, pid);
        id
    }

    /// A connection went away.
    ///
    /// Every handle anyone held to a node this process owned stops resolving,
    /// and whoever asked to hear about it gets a notice. That is the case
    /// reference counting exists for: a process can disappear at any moment.
    pub fn disconnect(&mut self, client: ClientId) -> Vec<DeathNotice> {
        let owned: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.owner == client)
            .map(|(node, _)| *node)
            .collect();

        let mut notices = Vec::new();
        for node in owned {
            for holder in self.holders_of(node) {
                if holder == client {
                    continue;
                }
                let Some(record) = self.clients.get_mut(&holder) else {
                    continue;
                };
                let linked = record.links.iter().any(|(_, owner)| *owner == client);
                let dropped = record.table.drop_owner(client);
                for (handle, _) in dropped {
                    if linked {
                        notices.push(DeathNotice {
                            client: holder,
                            handle,
                        });
                    }
                }
                record.links.retain(|(_, owner)| *owner != client);
            }
            self.nodes.remove(&node);
        }
        self.clients.remove(&client);
        log::debug!("Binder client {} disconnected", client);
        notices
    }

    /// Publish a name for a node this client owns.
    ///
    /// The node id carries the owner's pid in its high half, which is how a
    /// client is stopped from exporting a node it does not own: the kernel
    /// already told us which process this connection is.
    pub fn export(&mut self, client: ClientId, name: &str, node: u64) -> anyhow::Result<()> {
        let pid = self
            .clients
            .get(&client)
            .ok_or_else(|| anyhow::anyhow!("no such client: {}", client))?
            .pid;
        anyhow::ensure!(
            (node >> 32) as u32 == pid,
            "node {} does not belong to pid {}",
            node,
            pid
        );
        anyhow::ensure!(
            !self
                .nodes
                .values()
                .any(|existing| existing.name.as_deref() == Some(name)),
            "{} is already registered",
            name
        );

        self.nodes.insert(
            node,
            Node {
                owner: client,
                name: Some(name.to_string()),
            },
        );
        // The owner holds its own object, which is what keeps it alive.
        if let Some(record) = self.clients.get_mut(&client) {
            record.table.handle_for(Target::Local(node));
        }
        log::debug!("Binder: client {} exported {}", client, name);
        Ok(())
    }

    /// A handle in the caller's table for a name, if the broker knows one.
    pub fn lookup(&mut self, client: ClientId, name: &str) -> Option<(u32, u64, ClientId)> {
        let (node, owner) = self.node_named(name)?;
        let record = self.clients.get_mut(&client)?;
        // A process that owns a node reaches it directly. Asking for a name it
        // exported gives back the object it already has, not a proxy of itself.
        let target = if owner == client {
            Target::Local(node)
        } else {
            Target::Remote { node, owner }
        };
        let handle = record.table.handle_for(target);
        Some((handle, node, owner))
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .nodes
            .values()
            .filter_map(|node| node.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Whether a node exists and belongs to the process claiming to hold it,
    /// which is what makes a relayed answer trustworthy.
    pub fn accepts(&self, owner: ClientId, node: u64) -> bool {
        self.nodes
            .get(&node)
            .map(|entry| entry.owner == owner)
            .unwrap_or(false)
    }

    /// Resolve and dispatch. `data` is the parcel; the broker only passes it
    /// along or hands it to a hosted object.
    pub fn transact(
        &mut self,
        client: ClientId,
        handle: u32,
        code: u32,
        flags: u32,
        data: Vec<u8>,
    ) -> anyhow::Result<Dispatch> {
        let target = self
            .clients
            .get(&client)
            .ok_or_else(|| anyhow::anyhow!("no such client: {}", client))?
            .table
            .target(handle)
            .ok_or_else(|| anyhow::anyhow!("no handle {} in client {}", handle, client))?;

        match target {
            Target::Local(node) => Ok(Dispatch::Local { node }),
            Target::ServiceManager => anyhow::bail!("the service manager is the shim's to answer"),
            Target::Remote { node, owner } if owner == SERVICE_MANAGER => {
                Ok(self.hosted_reply(node, code, &data))
            }
            Target::Remote { node, owner } => Ok(Dispatch::Forward {
                owner,
                node,
                code,
                flags,
                data,
            }),
        }
    }

    /// A transaction to an object the broker itself hosts.
    fn hosted_reply(&mut self, node: u64, code: u32, data: &[u8]) -> Dispatch {
        let name = self.nodes.get(&node).and_then(|entry| entry.name.clone());
        match name {
            Some(name) => match self.hosted.transact(&name, code, data) {
                Ok(data) => Dispatch::Reply { status: 0, data },
                Err(e) => Dispatch::Reply {
                    status: -1,
                    data: e.to_string().into_bytes(),
                },
            },
            None => Dispatch::Reply {
                status: -1,
                data: b"no such object".to_vec(),
            },
        }
    }

    /// A weak reference taken by a client, as `BC_INCREFS` carries it.
    pub fn inc_refs(&mut self, client: ClientId, handle: u32) -> anyhow::Result<()> {
        self.table_mut(client)?.inc_refs(handle)
    }

    /// A strong reference taken, as `BC_ACQUIRE` carries it.
    pub fn acquire(&mut self, client: ClientId, handle: u32) -> anyhow::Result<()> {
        self.table_mut(client)?.acquire(handle).map(|_| ())
    }

    /// A reference dropped. When the last one goes the node goes with it,
    /// unless the broker hosts it.
    pub fn release(&mut self, client: ClientId, handle: u32) -> anyhow::Result<Option<u64>> {
        let released = self.table_mut(client)?.release(handle)?;
        let Released::Dropped(target) = released else {
            return Ok(None);
        };
        let Target::Remote { node, .. } = target else {
            return Ok(None);
        };
        self.collect(node);
        Ok(Some(node))
    }

    /// A weak reference dropped.
    pub fn dec_refs(&mut self, client: ClientId, handle: u32) -> anyhow::Result<Option<u64>> {
        let released = self.table_mut(client)?.dec_refs(handle)?;
        let Released::Dropped(target) = released else {
            return Ok(None);
        };
        if let Target::Remote { node, .. } = target {
            self.collect(node);
            return Ok(Some(node));
        }
        Ok(None)
    }

    /// Ask to be told when the owner of a handle goes away.
    pub fn link_to_death(&mut self, client: ClientId, handle: u32) -> anyhow::Result<()> {
        let owner = self
            .clients
            .get(&client)
            .and_then(|record| record.table.target(handle))
            .and_then(|target| target.owner())
            .ok_or_else(|| anyhow::anyhow!("handle {} has no owner", handle))?;
        let record = self
            .clients
            .get_mut(&client)
            .ok_or_else(|| anyhow::anyhow!("no such client: {}", client))?;
        record.links.retain(|(h, _)| *h != handle);
        record.links.push((handle, owner));
        Ok(())
    }

    pub fn unlink_to_death(&mut self, client: ClientId, handle: u32) -> anyhow::Result<()> {
        let record = self
            .clients
            .get_mut(&client)
            .ok_or_else(|| anyhow::anyhow!("no such client: {}", client))?;
        record.links.retain(|(h, _)| *h != handle);
        Ok(())
    }

    pub fn handle_count(&self, client: ClientId) -> usize {
        self.clients
            .get(&client)
            .map(|record| record.table.len())
            .unwrap_or(0)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn owner_of(&self, node: u64) -> Option<ClientId> {
        self.nodes.get(&node).map(|entry| entry.owner)
    }

    /// What a handle means in a given process.
    pub fn target_of(&self, client: ClientId, handle: u32) -> Option<Target> {
        self.clients.get(&client)?.table.target(handle)
    }

    fn node_named(&self, name: &str) -> Option<(u64, ClientId)> {
        self.nodes
            .iter()
            .find(|(_, node)| node.name.as_deref() == Some(name))
            .map(|(node, entry)| (*node, entry.owner))
    }

    /// Every client whose table holds a handle to this node.
    fn holders_of(&self, node: u64) -> Vec<ClientId> {
        let mut holders: Vec<ClientId> = self
            .clients
            .iter()
            .filter(|(_, record)| {
                record
                    .table
                    .handles()
                    .iter()
                    .any(|(_, target)| target.node() == Some(node))
            })
            .map(|(client, _)| *client)
            .collect();
        holders.sort_unstable();
        holders
    }

    fn table_mut(&mut self, client: ClientId) -> anyhow::Result<&mut HandleTable> {
        Ok(&mut self
            .clients
            .get_mut(&client)
            .ok_or_else(|| anyhow::anyhow!("no such client: {}", client))?
            .table)
    }

    /// Drop a node nobody can reach any more. A hosted node is the broker's,
    /// and stays until the broker says otherwise.
    fn collect(&mut self, node: u64) {
        let Some(entry) = self.nodes.get(&node) else {
            return;
        };
        if entry.owner == SERVICE_MANAGER {
            return;
        }
        if self.holders_of(node).is_empty() {
            self.nodes.remove(&node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(u32);

    impl BinderObject for Echo {
        fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Vec<u8>> {
            Ok([&self.0.to_be_bytes()[..], &code.to_be_bytes()[..], data].concat())
        }
    }

    fn node_for(pid: u32, counter: u32) -> u64 {
        ((pid as u64) << 32) | counter as u64
    }

    /// A2's gate: a service registered by name is found by name.
    #[test]
    fn a_registered_name_is_found_by_name() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(4242);
        let node = node_for(4242, 1);
        broker
            .export(client, "android.frameworks.stats.IStats/default", node)
            .unwrap();

        assert_eq!(
            broker.names(),
            vec!["android.frameworks.stats.IStats/default".to_string()]
        );

        let (handle, found_node, owner) = broker
            .lookup(client, "android.frameworks.stats.IStats/default")
            .unwrap();
        assert_eq!(found_node, node);
        assert_eq!(owner, client);
        assert_ne!(handle, 0, "the service manager keeps handle 0");

        assert!(broker.lookup(client, "nothing").is_none());
    }

    /// A name is one name: registering it twice is a mistake, not an override.
    #[test]
    fn a_name_can_only_be_exported_once() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(1000);
        broker.export(client, "svc", node_for(1000, 1)).unwrap();
        assert!(broker.export(client, "svc", node_for(1000, 2)).is_err());
    }

    /// A process cannot publish a node that belongs to another process.
    #[test]
    fn a_client_cannot_export_another_processs_node() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(1000);
        let err = broker
            .export(client, "stolen", node_for(2000, 1))
            .unwrap_err();
        assert!(err.to_string().contains("does not belong"));
    }

    /// The same object is a different number in each process, which is the
    /// whole reason handle tables exist: the number means nothing on its own.
    #[test]
    fn handles_are_per_process() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let other = broker.connect(2000);
        let node = node_for(1000, 7);
        broker.export(owner, "svc", node).unwrap();

        let (mine, _, _) = broker.lookup(owner, "svc").unwrap();
        let (theirs, _, _) = broker.lookup(other, "svc").unwrap();
        assert_eq!(
            broker.target_of(owner, mine),
            Some(Target::Local(node)),
            "the owner reaches its own object directly"
        );
        assert_eq!(
            broker.target_of(other, theirs),
            Some(Target::Remote { node, owner }),
            "another process reaches a proxy of it"
        );
        assert_eq!(broker.owner_of(node), Some(owner));
    }

    /// A hosted service answers where it is, with no forwarding at all.
    #[test]
    fn a_hosted_service_answers_in_place() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(1000);
        broker.host("system_suspend", Box::new(Echo(9)));

        let (handle, _, owner) = broker.lookup(client, "system_suspend").unwrap();
        assert_eq!(owner, SERVICE_MANAGER);

        match broker
            .transact(client, handle, 4, 0, b"in".to_vec())
            .unwrap()
        {
            Dispatch::Reply { status, data } => {
                assert_eq!(status, 0);
                assert_eq!(&data[..4], &9u32.to_be_bytes());
                assert_eq!(&data[4..8], &4u32.to_be_bytes());
                assert_eq!(&data[8..], b"in");
            }
            other => panic!("expected an in-place answer, got {:?}", other),
        }
    }

    /// A5's gate: a transaction to another process is named as a forward, with
    /// the owner to send it to.
    #[test]
    fn a_transaction_for_another_process_is_forwarded() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 3);
        broker.export(owner, "remote.svc", node).unwrap();

        let (handle, _, _) = broker.lookup(caller, "remote.svc").unwrap();
        match broker
            .transact(caller, handle, 11, 0, b"parcel".to_vec())
            .unwrap()
        {
            Dispatch::Forward {
                owner: routed,
                node: routed_node,
                code,
                data,
                ..
            } => {
                assert_eq!(routed, owner);
                assert_eq!(routed_node, node);
                assert_eq!(code, 11);
                assert_eq!(data, b"parcel");
            }
            other => panic!("expected a forward, got {:?}", other),
        }
    }

    /// A transaction to a handle the caller owns needs no broker at all, and
    /// saying so is what keeps a local call off the socket.
    #[test]
    fn a_local_handle_is_dispatched_by_its_owner() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(1000);
        let node = node_for(1000, 5);
        broker.export(client, "local.svc", node).unwrap();
        let (handle, _, _) = broker.lookup(client, "local.svc").unwrap();
        match broker.transact(client, handle, 1, 0, Vec::new()).unwrap() {
            Dispatch::Local { node: got } => assert_eq!(got, node),
            other => panic!("expected a local dispatch, got {:?}", other),
        }
    }

    /// A3's gate: a node survives one reference going away and is gone when the
    /// last one does, or when its owner goes.
    #[test]
    fn references_keep_a_node_alive() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 9);
        broker.export(owner, "svc", node).unwrap();

        let (handle, _, _) = broker.lookup(caller, "svc").unwrap();
        assert_eq!(broker.handle_count(caller), 2, "handle 0 and svc");

        broker.acquire(caller, handle).unwrap();
        broker.release(caller, handle).unwrap();
        assert_eq!(
            broker.handle_count(caller),
            2,
            "a reference remains, so the handle stays"
        );
        assert!(broker.transact(caller, handle, 1, 0, Vec::new()).is_ok());

        broker.release(caller, handle).unwrap();
        assert_eq!(
            broker.handle_count(caller),
            1,
            "the last reference went, so the handle is gone"
        );

        // The owner still holds its own object, so the name still resolves.
        assert_eq!(broker.owner_of(node), Some(owner));
        assert!(broker.lookup(caller, "svc").is_some());

        // The owner going away takes it with it.
        broker.disconnect(owner);
        assert_eq!(broker.owner_of(node), None);
        assert!(broker.lookup(caller, "svc").is_none());
    }

    /// The case A3 exists for: a process that dies tells everyone who asked.
    #[test]
    fn a_dead_owner_notifies_the_processes_using_it() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 11);
        broker.export(owner, "svc", node).unwrap();

        let (handle, _, _) = broker.lookup(caller, "svc").unwrap();
        broker.link_to_death(caller, handle).unwrap();

        let notices = broker.disconnect(owner);
        assert_eq!(
            notices,
            vec![DeathNotice {
                client: caller,
                handle
            }]
        );
        assert!(
            broker.transact(caller, handle, 1, 0, Vec::new()).is_err(),
            "a handle to a dead process must not resolve"
        );
    }

    /// A client that never asked is not told, but its handle stops resolving
    /// all the same.
    #[test]
    fn an_unlinked_client_is_not_notified() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 12);
        broker.export(owner, "svc", node).unwrap();
        let (handle, _, _) = broker.lookup(caller, "svc").unwrap();

        assert!(broker.disconnect(owner).is_empty(), "nobody asked");
        assert!(broker.transact(caller, handle, 1, 0, Vec::new()).is_err());

        // And unlink means the linker stops hearing about it too.
        let owner = broker.connect(1001);
        let node = node_for(1001, 13);
        broker.export(owner, "svc2", node).unwrap();
        let (handle, _, _) = broker.lookup(caller, "svc2").unwrap();
        broker.link_to_death(caller, handle).unwrap();
        broker.unlink_to_death(caller, handle).unwrap();
        assert!(broker.disconnect(owner).is_empty());
    }

    /// A relayed answer is only accepted from the process that owns the node.
    #[test]
    fn an_answer_is_only_taken_from_the_owner() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let stranger = broker.connect(3000);
        let node = node_for(1000, 14);
        broker.export(owner, "svc", node).unwrap();
        assert!(broker.accepts(owner, node));
        assert!(!broker.accepts(stranger, node));
        assert!(!broker.accepts(owner, node_for(1000, 15)));
    }

    /// A client that goes away takes its handles with it, and the accounts on
    /// the nodes it was using settle.
    #[test]
    fn disconnect_settles_the_accounts() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 16);
        broker.export(owner, "svc", node).unwrap();
        let (handle, _, _) = broker.lookup(caller, "svc").unwrap();
        assert!(broker.transact(caller, handle, 1, 0, Vec::new()).is_ok());

        broker.disconnect(caller);
        assert_eq!(
            broker.owner_of(node),
            Some(owner),
            "the owner keeps its own"
        );
        assert!(broker.transact(caller, handle, 1, 0, Vec::new()).is_err());
    }

    #[test]
    fn a_weak_reference_does_not_outlive_a_strong_one() {
        let mut broker = BinderBroker::new();
        let owner = broker.connect(1000);
        let caller = broker.connect(2000);
        let node = node_for(1000, 17);
        broker.export(owner, "svc", node).unwrap();
        let (handle, _, _) = broker.lookup(caller, "svc").unwrap();

        broker.inc_refs(caller, handle).unwrap();
        broker.dec_refs(caller, handle).unwrap();
        assert_eq!(broker.handle_count(caller), 2);
        broker.release(caller, handle).unwrap();
        assert_eq!(broker.handle_count(caller), 1, "only the service manager");
    }

    /// A hosted service is the broker's and outlives every client.
    #[test]
    fn a_hosted_service_is_not_collected() {
        let mut broker = BinderBroker::new();
        let client = broker.connect(1000);
        broker.host("svc", Box::new(Echo(1)));
        let (handle, _, _) = broker.lookup(client, "svc").unwrap();
        broker.release(client, handle).unwrap();
        broker.disconnect(client);
        let next = broker.connect(1001);
        assert!(broker.lookup(next, "svc").is_some());
    }
}
