// SPDX-License-Identifier: GPL-3.0-or-later

//! The broker's side of the Binder data plane (A5).
//!
//! One thread per connection. A transaction to a service in another process is
//! sent to that process as [`Message::Incoming`] and its answer is relayed
//! back, which is what makes a call cross a process boundary at all. The
//! waiting is deliberate: a synchronous transaction blocks its thread, exactly
//! as it does on a device, and for the same reason -- the caller has nothing
//! else to do until the answer arrives.

use crate::binder::broker::{BinderBroker, ClientId, Dispatch};
use crate::binder::table::Target;
use crate::binder::wire::{
    ArgumentRef, Conn, Frame, Message, Reader, Writer, NO_HANDLE, TF_ONE_WAY,
};
use crate::binder::BinderObject;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::Duration;

/// How long a forwarded transaction waits for the process that owns the
/// object. A process that has stopped answering must not hold a caller
/// forever.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(30);

/// The broker's Binder state and the connections attached to it.
pub struct Transport {
    broker: Mutex<BinderBroker>,
    /// Every connection a client has, because one process may have more than one
    /// and they share a handle table. Notices and forwards go to all of them; a
    /// reply goes to the one that asked, which is carried with the call.
    peers: Mutex<HashMap<ClientId, Vec<Arc<Peer>>>>,
    /// Where an answer from a process goes when it comes back. Keyed by the
    /// process that was asked, the node it was asked about, *and the request*:
    /// one node can have several transactions in flight, and a node on its own
    /// cannot tell one answer from another.
    /// An in-flight forwarded call: who asked, and where their answer goes. The
    /// caller is here because the answer may carry objects, and a handle is only
    /// meaningful beside the table it belongs to.
    pending: Mutex<Pending>,
}

/// In-flight forwarded calls, by the node, the owner and the request.
type Pending = HashMap<(ClientId, u64, u32), (ClientId, Arc<Peer>, SyncSender<Frame>)>;

struct Peer {
    writer: Mutex<Writer>,
}

/// One transaction as the caller sent it, kept together because it travels
/// through the broker and then to whichever process owns the object.
struct Call {
    /// The caller's name for this request, echoed back with the answer.
    id: u32,
    handle: u32,
    code: u32,
    flags: u32,
    data: Vec<u8>,
    /// Objects among the arguments, which the broker resolves into the owner's
    /// handles before the call is forwarded.
    objects: Vec<ArgumentRef>,
    fds: Vec<std::os::fd::OwnedFd>,
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport {
    pub fn new() -> Self {
        Self {
            broker: Mutex::new(BinderBroker::new()),
            peers: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Host an object in the broker, so every process can reach it without a
    /// second process existing to provide it.
    pub fn host(&self, name: &str, object: Box<dyn BinderObject>) -> u64 {
        self.broker.lock().host(name, object)
    }

    /// The names currently registered, for the control plane and for logging.
    pub fn names(&self) -> Vec<String> {
        self.broker.lock().names()
    }

    /// Serve one connection until it goes away. Blocking: the caller gives it a
    /// thread of its own.
    pub fn serve(&self, stream: &std::os::unix::net::UnixStream) -> anyhow::Result<()> {
        let fd = stream.as_raw_fd();
        let pid = peer_pid(fd)?;
        let (reader, writer) = Conn::new(fd).split();
        let client = self.broker.lock().connect(pid);
        let peer = Arc::new(Peer {
            writer: Mutex::new(writer),
        });
        self.peers
            .lock()
            .entry(client)
            .or_default()
            .push(peer.clone());

        let result = self.session(client, peer.clone(), reader);

        // Only if this connection still owns the client. A process that reconnects
        // while this session is unwinding has a newer peer in the map, and tearing
        // down here would mark the *live* connection dead: every call it makes is
        // then refused, which is the same failure the reconnect fix was for. The
        // peer is the identity of a connection, so that is what is compared.
        let mine = self
            .peers
            .lock()
            .get(&client)
            .is_some_and(|current| current.iter().any(|existing| Arc::ptr_eq(existing, &peer)));
        if !mine {
            log::debug!(
                "Binder session for client {} ended after it reconnected",
                client
            );
            return result;
        }

        let notices = self.broker.lock().disconnect(client);
        // Only this connection goes: the process may have another one open, and
        // removing the client outright would take it with it.
        self.peers.lock().retain(|_, peers| {
            peers.retain(|existing| !Arc::ptr_eq(existing, &peer));
            !peers.is_empty()
        });
        self.pending
            .lock()
            .retain(|(owner, _, _), _| *owner != client);
        for notice in notices {
            let targets = self.peers.lock().get(&notice.client).cloned();
            for target in targets.into_iter().flatten() {
                let _ = target.writer.lock().send(
                    &Message::Dead {
                        handle: notice.handle,
                    },
                    &[],
                );
            }
        }
        log::debug!("Binder session for client {} ended", client);
        if let Err(e) = &result {
            log::warn!(
                "Binder session for client {} ended with an error: {}",
                client,
                e
            );
        }
        result
    }

    /// Serve one connection.
    ///
    /// `peer` is *this* connection's writer, and it is passed rather than looked up
    /// because a process may have more than one. The peer map holds the latest, so
    /// answering through it sent a reply to whichever connection came last -- and a
    /// process with two of them heard nothing on the one it asked from.
    #[allow(clippy::too_many_arguments)]
    fn session(&self, client: ClientId, peer: Arc<Peer>, mut reader: Reader) -> anyhow::Result<()> {
        loop {
            let frame = match reader.recv() {
                Ok(frame) => frame,
                Err(e) => {
                    // Which way a connection ends matters and is not visible from
                    // the outside: a peer that closed looks the same in the log as
                    // a frame this side could not read, and the fix is different
                    // for each.
                    log::warn!("Binder client {}: reading failed: {}", client, e);
                    return Err(e);
                }
            };
            match frame.message {
                Message::Bye => {
                    log::debug!("Binder client {} said goodbye", client);
                    return Ok(());
                }

                Message::Transaction {
                    id,
                    handle,
                    code,
                    flags,
                    data,
                    objects,
                } => {
                    let call = Call {
                        id,
                        handle,
                        code,
                        flags,
                        data,
                        objects,
                        fds: frame.fds,
                    };
                    // `id` and `flags` are copied out because the call itself
                    // carries descriptors, which cannot be duplicated -- and the
                    // failed reply needs both after the call has been consumed.
                    // Any failure to answer a transaction is the caller's problem,
                    // not the connection's. This is the one place that decides it,
                    // and it matters for more than a handle that does not route: a
                    // *forwarded* call whose owner has died waits out the timeout
                    // and fails, and when that error ended the session it took the
                    // whole connection with it -- one dead service cost the caller
                    // every other service it was using, for the rest of its life.
                    // A failed reply is what a driver gives, and the session lives.
                    if let Err(e) = self.transaction(client, peer.clone(), call) {
                        log::warn!("client {}: a transaction failed: {}", client, e);
                        if flags & TF_ONE_WAY == 0 {
                            self.to_peer(
                                client,
                                Message::Reply {
                                    id,
                                    status: -129, // EX_TRANSACTION_FAILED
                                    data: e.to_string().into_bytes(),
                                    objects: Vec::new(),
                                },
                                &[],
                            )?;
                        }
                    }
                }

                // Matched by the request it answers, not by the node: one node
                // can have several transactions in flight, and the node alone
                // cannot tell one answer from another.
                Message::IncomingReply {
                    id,
                    node,
                    status,
                    data,
                    objects,
                } => {
                    let waiter = self.pending.lock().remove(&(client, node, id));
                    match waiter {
                        Some((caller, _asked_from, waiter)) => {
                            let (data, objects) = self.reply_objects(client, caller, data, objects);
                            let _ = waiter.send(Frame {
                                message: Message::Reply {
                                    id,
                                    status,
                                    data,
                                    objects,
                                },
                                fds: frame.fds,
                            });
                        }
                        None => log::debug!(
                            "client {} answered node {} with nobody waiting for request {}",
                            client,
                            node,
                            id
                        ),
                    }
                }

                Message::Acquire { handle } => {
                    self.broker.lock().acquire(client, handle)?;
                }
                Message::Release { handle } => {
                    let released = self.broker.lock().release(client, handle)?;
                    log::info!(
                        "client {} released handle {} ({:?})",
                        client,
                        handle,
                        released
                    );
                }
                Message::IncRefs { handle } => {
                    self.broker.lock().inc_refs(client, handle)?;
                }
                Message::DecRefs { handle } => {
                    self.broker.lock().dec_refs(client, handle)?;
                }

                Message::LinkToDeath { handle } => {
                    self.broker.lock().link_to_death(client, handle)?;
                }
                Message::UnlinkToDeath { handle } => {
                    self.broker.lock().unlink_to_death(client, handle)?;
                }

                Message::Export { name, node } => {
                    match self.broker.lock().export(client, &name, node) {
                        Ok(()) => {
                            log::debug!("client {} exported {} as node {}", client, name, node)
                        }
                        Err(e) => log::warn!("client {} could not export {}: {}", client, name, e),
                    }
                }

                Message::Lookup { id, name } => {
                    // Logged because a lookup that never arrives and a lookup that
                    // answers "no such name" look identical from the framework's
                    // side, and telling them apart is the whole question.
                    log::info!("client {} looked up {}", client, name);
                    let found = self.broker.lock().lookup(client, &name);
                    let answer = match found {
                        Some((handle, node, owner)) => Message::Found {
                            id,
                            handle,
                            node,
                            owner,
                        },
                        None => Message::Found {
                            id,
                            handle: NO_HANDLE,
                            node: 0,
                            owner: 0,
                        },
                    };
                    // An answer, so it goes back the way the question came. A
                    // process with two connections would otherwise hear it on
                    // whichever one came last, and the one that asked would wait.
                    peer.writer.lock().send(&answer, &[])?;
                }

                other => anyhow::bail!("a client may not send {:?}", other),
            }
        }
    }

    /// The objects a callee answered with, rewritten for the caller.
    ///
    /// Each one is a handle in the *callee's* table, and the caller has its own:
    /// passing the number through would point at whatever happens to sit at that
    /// number there, which is worse than failing. This looks each one up and mints
    /// the caller's own handle for it.
    fn reply_objects(
        &self,
        callee: ClientId,
        caller: ClientId,
        mut data: Vec<u8>,
        objects: Vec<u32>,
    ) -> (Vec<u8>, Vec<u32>) {
        let mut broker = self.broker.lock();
        let mut rewritten = Vec::with_capacity(objects.len());
        for offset in objects {
            let at = offset as usize;
            if at + 28 > data.len() {
                continue;
            }
            let handle = u32::from_le_bytes(data[at + 8..at + 12].try_into().unwrap_or([0; 4]));
            let node = match broker.target_of(callee, handle) {
                Some(Target::Remote { node, .. }) | Some(Target::Local(node)) => node,
                _ => continue,
            };
            let Some(caller_handle) = broker.handle_for(caller, node) else {
                continue;
            };
            crate::binder::broker::write_handle(&mut data, at, caller_handle, node);
            rewritten.push(offset);
        }
        (data, rewritten)
    }

    fn transaction(&self, client: ClientId, peer: Arc<Peer>, call: Call) -> anyhow::Result<()> {
        let dispatch = match self.broker.lock().transact(
            client,
            call.handle,
            call.code,
            call.flags,
            call.data.clone(),
            call.objects.clone(),
        ) {
            Ok(dispatch) => dispatch,
            // A transaction the broker cannot route is the caller's problem, not
            // the connection's. Failing the session here -- which is what this did
            // -- closed the socket, and every later call from that process then
            // failed with "sending to the broker failed" for the rest of its life:
            // one unroutable handle cost the whole process its transport. The
            // answer is a failed transaction, which is what a driver gives.
            Err(e) => {
                log::warn!(
                    "client {} sent a transaction that cannot be routed: {}",
                    client,
                    e
                );
                return Err(e);
            }
        };

        match dispatch {
            Dispatch::Reply {
                status,
                data,
                objects,
                fds,
                calls,
            } => {
                if call.flags & TF_ONE_WAY == 0 {
                    // The descriptors go with the frame: the answer says where
                    // their words are, and the frame carries the descriptors
                    // themselves, which is how they cross into the caller's
                    // process at all.
                    let raw: Vec<RawFd> = fds.iter().map(|(_, fd)| fd.as_raw_fd()).collect();
                    self.to_peer(
                        client,
                        Message::Reply {
                            id: call.id,
                            status,
                            data,
                            objects,
                        },
                        &raw,
                    )?;
                }
                // What the service asked to have called, now that its answer is
                // on its way. The handle is in the caller's table, so this is a
                // transaction *to* the caller, and it goes the same way any
                // forwarded call does -- which is what makes a callback
                // registration work: the framework hands the HAL an object, the
                // HAL calls it, and the framework's own Binder thread serves it.
                for pending in calls {
                    // A call *to* a client rather than a request from one, so it goes
                    // as `Incoming` -- the kind the receiving side runs on its own
                    // binder thread. Sending it as a transaction, which is what this
                    // did, puts it in the half of the protocol the receiver only ever
                    // writes to, and nothing reads it there: the callback the service
                    // asked for is never made, and `BatteryService` waits sixty
                    // seconds a boot for a health registration it has already been
                    // promised.
                    //
                    // The handle belongs to the caller's table, so the owner has to be
                    // found through it -- the object may be the caller's own or one it
                    // was handed a proxy for.
                    let Some((owner, node)) =
                        self.broker.lock().call_target(client, pending.handle)
                    else {
                        log::warn!(
                            "client {} asked for a call on handle {}, which is not in its table",
                            client,
                            pending.handle
                        );
                        continue;
                    };
                    // One-way, which is what a callback is: nothing waits for an
                    // answer, so the id is the request's and means nothing to the
                    // caller.
                    self.to_peer(
                        owner,
                        Message::Incoming {
                            id: call.id,
                            node,
                            code: pending.code,
                            flags: TF_ONE_WAY,
                            data: pending.data,
                            objects: Vec::new(),
                        },
                        &[],
                    )?;
                }
            }
            Dispatch::Local { node } => {
                // The caller owns this object and can call it without us.
                // Saying so beats leaving it waiting for an answer.
                log::debug!(
                    "client {} transacted on node {}, which it owns",
                    client,
                    node
                );
                if call.flags & TF_ONE_WAY == 0 {
                    self.to_peer(
                        client,
                        Message::Reply {
                            id: call.id,
                            status: -1,
                            data: b"a local object is called directly".to_vec(),
                            objects: Vec::new(),
                        },
                        &[],
                    )?;
                }
            }
            Dispatch::Forward { owner, node, .. } => {
                self.forward(client, peer.clone(), owner, node, call)?
            }
        }
        Ok(())
    }

    /// Ask the process that owns a node to run a transaction, and relay the
    /// answer to whoever asked.
    #[allow(clippy::too_many_arguments)]
    fn forward(
        &self,
        caller: ClientId,
        caller_peer: Arc<Peer>,
        owner: ClientId,
        node: u64,
        call: Call,
    ) -> anyhow::Result<()> {
        let (sender, receiver) = sync_channel(1);
        // The connection the caller asked from is remembered, not looked up: the
        // answer has to go back the way the call came.
        self.pending.lock().insert(
            (owner, node, call.id),
            (caller, caller_peer.clone(), sender),
        );
        let descriptors: Vec<RawFd> = call.fds.iter().map(|fd| fd.as_raw_fd()).collect();
        self.to_peer(
            owner,
            Message::Incoming {
                id: call.id,
                node,
                code: call.code,
                flags: call.flags,
                data: call.data,
                objects: call.objects.iter().map(|object| object.offset).collect(),
            },
            &descriptors,
        )?;

        if call.flags & TF_ONE_WAY != 0 {
            self.pending.lock().remove(&(owner, node, call.id));
            return Ok(());
        }

        let answer = receiver.recv_timeout(FORWARD_TIMEOUT);
        self.pending.lock().remove(&(owner, node, call.id));
        let answer = answer
            .map_err(|_| anyhow::anyhow!("no answer for node {} from client {}", node, owner))?;
        let fds: Vec<RawFd> = answer.fds.iter().map(|fd| fd.as_raw_fd()).collect();
        // The writer's lock is taken after the map's is dropped, as everywhere
        // else, so a slow connection cannot hold up everyone's routing.
        caller_peer.writer.lock().send(&answer.message, &fds)
    }

    fn to_peer(&self, client: ClientId, message: Message, fds: &[RawFd]) -> anyhow::Result<()> {
        let peers = self.peers.lock().get(&client).cloned().unwrap_or_default();
        anyhow::ensure!(!peers.is_empty(), "no connection for client {}", client);
        // Every connection the client has, because they share a handle table: a
        // transaction forwarded to the process is served by whichever connection
        // reads it. The lock on each writer is taken after the map's is dropped, so
        // a slow connection cannot hold up everyone else's routing.
        for peer in &peers {
            peer.writer.lock().send(&message, fds)?;
        }
        Ok(())
    }
}

/// The pid of the process on the other end, from the kernel rather than from
/// anything the client says about itself.
/// Which process a connection belongs to.
///
/// In a test every connection comes from the test's own pid, so two connections
/// that are meant to be two *processes* look like one -- which is now a real
/// difference, since a process is one client however many connections it has.
/// Tests set this; everything else asks the kernel.
#[cfg(test)]
static PEER_PIDS: Mutex<Option<HashMap<RawFd, u32>>> = Mutex::new(None);

/// Say which process a connection is.
///
/// Tests only, and only because every connection in a test comes from the test's
/// own pid: two connections meant to be two *processes* cannot say so otherwise,
/// and since a process is one client however many connections it has, that
/// difference is now one the tests need to express. Keyed by descriptor, so tests
/// running in parallel do not walk on each other.
#[cfg(test)]
pub fn set_peer_pid_for_tests(fd: RawFd, pid: u32) {
    PEER_PIDS
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(fd, pid);
}

fn peer_pid(fd: RawFd) -> anyhow::Result<u32> {
    #[cfg(test)]
    {
        if let Some(pids) = PEER_PIDS.lock().as_ref() {
            if let Some(pid) = pids.get(&fd) {
                return Ok(*pid);
            }
        }
    }
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    // SAFETY: the descriptor is open for as long as the caller holds the
    // stream, which outlives this call.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let credentials = getsockopt(&borrowed, PeerCredentials)?;
    Ok(credentials.pid() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::wire::MAX_FDS;
    use crate::binder::Answer;
    use std::os::unix::net::{UnixListener, UnixStream};

    struct Echo;

    impl BinderObject for Echo {
        fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Answer> {
            Ok([b"echo:".as_slice(), &code.to_be_bytes(), data]
                .concat()
                .into())
        }
    }

    /// A transport with two connections to it, each served on its own thread.
    /// That is the shape of two processes talking to the broker: the transport
    /// cannot tell whether the other end is a thread or a process.
    struct Harness {
        /// Declared first so the descriptors outlive the connections that use
        /// them.
        streams: (UnixStream, UnixStream),
        transport: Arc<Transport>,
        owner: Conn,
        caller: Conn,
        _dir: tempfile::TempDir,
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binder.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let transport = Arc::new(Transport::new());
        // A hang-up in these tests stands for a process that died, which is what
        // they are about. Deciding liveness here rather than reading `/proc` keeps
        // them from depending on which pids the machine happens to be using -- and
        // a test that wants the *other* answer says so.
        transport.broker.lock().set_liveness(|_| false);

        let acceptor = transport.clone();
        std::thread::spawn(move || {
            for n in 0..2u32 {
                let (stream, _) = listener.accept().unwrap();
                // Two connections, two processes: they share a handle table only
                // when they come from one, and these tests are about what crosses
                // between two. The pid is set on the socket the server side holds,
                // because that is the one `peer_pid` is asked about.
                set_peer_pid_for_tests(stream.as_raw_fd(), 1001 + n);
                let transport = acceptor.clone();
                std::thread::spawn(move || {
                    let _ = transport.serve(&stream);
                });
            }
        });

        let owner_stream = connect(&path);
        let caller_stream = connect(&path);
        let owner = Conn::new(owner_stream.as_raw_fd());
        let caller = Conn::new(caller_stream.as_raw_fd());
        Harness {
            streams: (owner_stream, caller_stream),
            transport,
            owner,
            caller,
            _dir: dir,
        }
    }

    /// An object the broker hosts that answers with a descriptor, which is the
    /// shape a display event connection has.
    struct HandingDescriptor;

    impl BinderObject for HandingDescriptor {
        fn transact(&mut self, _code: u32, _data: &[u8]) -> anyhow::Result<Answer> {
            let (a, _b) = nix::sys::socket::socketpair(
                nix::sys::socket::AddressFamily::Unix,
                nix::sys::socket::SockType::SeqPacket,
                None,
                nix::sys::socket::SockFlag::SOCK_CLOEXEC,
            )?;
            let mut reply = crate::binder::parcel::Parcel::new();
            reply.ok();
            let offset = reply.fd_placeholder();
            Ok(Answer::from(reply.into_bytes()).handing_fd(offset, a))
        }
    }

    #[test]
    fn a_reply_can_carry_a_descriptor() {
        let mut h = harness();
        let node = h
            .transport
            .host("descriptor.svc", Box::new(HandingDescriptor));
        let handle = find(&mut h.caller, "descriptor.svc");
        assert!(node != 0 && handle != u32::MAX);

        h.caller
            .send(
                &Message::Transaction {
                    id: 1,
                    handle,
                    code: 1,
                    flags: 0,
                    data: Vec::new(),
                    objects: Vec::new(),
                },
                &[],
            )
            .unwrap();
        let frame = h.caller.recv().unwrap();
        // The descriptor crosses with the frame: the word in the data says where it
        // belongs, and the frame carries the descriptor itself.
        assert_eq!(frame.fds.len(), 1, "the answer carried a descriptor");
        let Message::Reply { data, objects, .. } = frame.message else {
            panic!("expected an answer")
        };
        assert_eq!(objects, vec![4]);
        // The status, then a descriptor object: 24 bytes, since a descriptor
        // carries no stability word.
        assert!(data.len() >= 4 + 24);
    }

    fn connect(path: &std::path::Path) -> UnixStream {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match UnixStream::connect(path) {
                Ok(stream) => return stream,
                Err(e) if std::time::Instant::now() < deadline => {
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("could not connect: {}", e),
            }
        }
    }

    /// Resolve a name, retrying: two connections are served concurrently, so a
    /// lookup can beat the export that precedes it.
    fn find(conn: &mut Conn, name: &str) -> u32 {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            conn.send(
                &Message::Lookup {
                    id: 1,
                    name: name.to_string(),
                },
                &[],
            )
            .unwrap();
            match conn.recv().unwrap().message {
                Message::Found { handle, .. } if handle != NO_HANDLE => return handle,
                Message::Found { .. } => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "{} never appeared",
                        name
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("expected Found, got {:?}", other),
            }
        }
    }

    /// Wait until the broker has handled everything already sent on this
    /// connection, by asking a question whose answer is cheap and empty.
    fn settle(conn: &mut Conn) {
        conn.send(
            &Message::Lookup {
                id: 2,
                name: "nothing.sync".to_string(),
            },
            &[],
        )
        .unwrap();
        match conn.recv().unwrap().message {
            Message::Found { handle, .. } => assert_eq!(handle, NO_HANDLE),
            other => panic!("expected Found, got {:?}", other),
        }
    }

    /// A node belonging to one of the harness's two processes. The node id carries
    /// its owner's pid, which is how the broker stops a client exporting another's
    /// object -- so the pid here has to be the one the harness told the connection
    /// it was, not the test's own.
    fn own_node(pid: u32, counter: u32) -> u64 {
        ((pid as u64) << 32) | counter as u64
    }

    #[test]
    fn the_peer_pid_comes_from_the_kernel() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_pid(a.as_raw_fd()).unwrap(), std::process::id());
    }

    #[test]
    fn a_hosted_object_answers_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binder.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let transport = Arc::new(Transport::new());
        transport.host("echo", Box::new(Echo));

        let acceptor = transport.clone();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = acceptor.serve(&stream);
        });

        let stream = connect(&path);
        let mut conn = Conn::new(stream.as_raw_fd());
        let handle = find(&mut conn, "echo");
        assert_ne!(handle, NO_HANDLE);

        conn.send(
            &Message::Transaction {
                id: 1,
                handle,
                code: 7,
                flags: 0,
                data: b"parcel".to_vec(),
                objects: Vec::new(),
            },
            &[],
        )
        .unwrap();
        match conn.recv().unwrap().message {
            Message::Reply {
                id: 1,
                status,
                data,
                ..
            } => {
                assert_eq!(status, 0);
                assert_eq!(&data[..5], b"echo:");
                assert_eq!(&data[5..9], &7u32.to_be_bytes());
                assert_eq!(&data[9..], b"parcel");
            }
            other => panic!("expected Reply, got {:?}", other),
        }
        assert!(transport.names().contains(&"echo".to_string()));
    }

    /// A5's gate, end to end and over real sockets: a name exported by one
    /// connection is found and called from another, and the answer comes back.
    #[test]
    fn a_transaction_crosses_between_two_connections() {
        let mut h = harness();
        let node = own_node(1001, 42);
        h.owner
            .send(
                &Message::Export {
                    name: "owner.svc".to_string(),
                    node,
                },
                &[],
            )
            .unwrap();

        let handle = find(&mut h.caller, "owner.svc");

        h.caller
            .send(
                &Message::Transaction {
                    id: 1,
                    handle,
                    code: 3,
                    flags: 0,
                    data: b"who is it".to_vec(),
                    objects: Vec::new(),
                },
                &[],
            )
            .unwrap();

        let id = match h.owner.recv().unwrap().message {
            Message::Incoming {
                id,
                node: asked,
                code,
                data,
                ..
            } => {
                assert_eq!(asked, node);
                assert_eq!(code, 3);
                assert_eq!(data, b"who is it");
                id
            }
            other => panic!("expected Incoming, got {:?}", other),
        };
        assert!(
            id > 0,
            "the request is named so the answer can be matched to it"
        );

        h.owner
            .send(
                &Message::IncomingReply {
                    id,
                    node,
                    status: 0,
                    data: b"the owner".to_vec(),
                    objects: Vec::new(),
                },
                &[],
            )
            .unwrap();

        match h.caller.recv().unwrap().message {
            Message::Reply {
                id: 1,
                status,
                data,
                ..
            } => {
                assert_eq!(status, 0);
                assert_eq!(data, b"the owner");
            }
            other => panic!("expected Reply, got {:?}", other),
        }
    }

    /// A descriptor passed in a transaction is forwarded with it, so a file
    /// keeps its identity across the broker (A4 at transport level).
    #[test]
    fn descriptors_are_forwarded_between_connections() {
        use std::io::{Read, Seek, SeekFrom, Write};

        let mut h = harness();
        let node = own_node(1001, 77);
        h.owner
            .send(
                &Message::Export {
                    name: "fd.svc".to_string(),
                    node,
                },
                &[],
            )
            .unwrap();
        let handle = find(&mut h.caller, "fd.svc");

        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"through the broker").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        h.caller
            .send(
                &Message::Transaction {
                    id: 1,
                    handle,
                    code: 1,
                    flags: 0,
                    data: Vec::new(),
                    objects: Vec::new(),
                },
                &[file.as_raw_fd()],
            )
            .unwrap();

        let frame = h.owner.recv().unwrap();
        assert!(matches!(frame.message, Message::Incoming { .. }));
        assert_eq!(frame.fds.len(), 1, "the descriptor came with the call");
        assert!(frame.fds.len() <= MAX_FDS);

        let mut received = std::fs::File::from(frame.fds.into_iter().next().unwrap());
        let mut contents = String::new();
        received.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "through the broker");

        h.owner
            .send(
                &Message::IncomingReply {
                    id: 1,
                    node,
                    status: 0,
                    data: Vec::new(),
                    objects: Vec::new(),
                },
                &[],
            )
            .unwrap();
        match h.caller.recv().unwrap().message {
            Message::Reply { status, .. } => assert_eq!(status, 0),
            other => panic!("expected Reply, got {:?}", other),
        }
    }

    /// A caller that asked to hear about a service dying is told when the
    /// process providing it goes away.
    #[test]
    fn a_client_that_goes_away_is_reported() {
        let mut h = harness();
        let node = own_node(1001, 99);
        h.owner
            .send(
                &Message::Export {
                    name: "doomed.svc".to_string(),
                    node,
                },
                &[],
            )
            .unwrap();
        let handle = find(&mut h.caller, "doomed.svc");

        h.caller
            .send(&Message::LinkToDeath { handle }, &[])
            .unwrap();
        settle(&mut h.caller);

        // The owner hangs up, the way a process that dies does.
        h.streams.0.shutdown(std::net::Shutdown::Both).unwrap();

        match h.caller.recv().unwrap().message {
            Message::Dead { handle: dead } => assert_eq!(dead, handle),
            other => panic!("expected Dead, got {:?}", other),
        }
    }

    /// A caller that transacts with a service the broker hosts itself gets an
    /// answer without any forwarding: the same path the suspend HAL will use.
    #[test]
    fn a_hosted_service_is_reachable_by_name() {
        let mut h = harness();
        h.transport.host("suspend_control_internal", Box::new(Echo));
        let handle = find(&mut h.caller, "suspend_control_internal");
        h.caller
            .send(
                &Message::Transaction {
                    id: 1,
                    handle,
                    code: 1,
                    flags: 0,
                    data: b"register".to_vec(),
                    objects: Vec::new(),
                },
                &[],
            )
            .unwrap();
        match h.caller.recv().unwrap().message {
            Message::Reply {
                id: 1,
                status,
                data,
                ..
            } => {
                assert_eq!(status, 0);
                assert!(data.starts_with(b"echo:"));
            }
            other => panic!("expected Reply, got {:?}", other),
        }
    }
}
