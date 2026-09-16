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
use crate::binder::wire::{Conn, Frame, Message, Reader, Writer, NO_HANDLE, TF_ONE_WAY};
use crate::binder::BinderObject;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a forwarded transaction waits for the process that owns the
/// object. A process that has stopped answering must not hold a caller
/// forever.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(30);

/// The broker's Binder state and the connections attached to it.
pub struct Transport {
    broker: Mutex<BinderBroker>,
    peers: Mutex<HashMap<ClientId, Arc<Peer>>>,
    /// Where an answer from a process goes when it comes back. Keyed by the
    /// process that was asked and the node it was asked about, because one
    /// connection runs one transaction at a time.
    pending: Mutex<HashMap<(ClientId, u64), SyncSender<Frame>>>,
}

struct Peer {
    writer: Mutex<Writer>,
}

/// One transaction as the caller sent it, kept together because it travels
/// through the broker and then to whichever process owns the object.
struct Call {
    handle: u32,
    code: u32,
    flags: u32,
    data: Vec<u8>,
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
        self.broker.lock().unwrap().host(name, object)
    }

    /// The names currently registered, for the control plane and for logging.
    pub fn names(&self) -> Vec<String> {
        self.broker.lock().unwrap().names()
    }

    /// Serve one connection until it goes away. Blocking: the caller gives it a
    /// thread of its own.
    pub fn serve(&self, stream: &std::os::unix::net::UnixStream) -> anyhow::Result<()> {
        let fd = stream.as_raw_fd();
        let pid = peer_pid(fd)?;
        let (reader, writer) = Conn::new(fd).split();
        let client = self.broker.lock().unwrap().connect(pid);
        let peer = Arc::new(Peer {
            writer: Mutex::new(writer),
        });
        self.peers.lock().unwrap().insert(client, peer);

        let result = self.session(client, reader);

        let notices = self.broker.lock().unwrap().disconnect(client);
        self.peers.lock().unwrap().remove(&client);
        self.pending
            .lock()
            .unwrap()
            .retain(|(owner, _), _| *owner != client);
        for notice in notices {
            let target = self.peers.lock().unwrap().get(&notice.client).cloned();
            if let Some(target) = target {
                let _ = target.writer.lock().unwrap().send(
                    &Message::Dead {
                        handle: notice.handle,
                    },
                    &[],
                );
            }
        }
        log::debug!("Binder session for client {} ended", client);
        result
    }

    fn session(&self, client: ClientId, mut reader: Reader) -> anyhow::Result<()> {
        loop {
            let frame = reader.recv()?;
            match frame.message {
                Message::Bye => return Ok(()),

                Message::Transaction {
                    handle,
                    code,
                    flags,
                    data,
                } => self.transaction(
                    client,
                    Call {
                        handle,
                        code,
                        flags,
                        data,
                        fds: frame.fds,
                    },
                )?,

                Message::IncomingReply { node, status, data } => {
                    let waiter = self.pending.lock().unwrap().remove(&(client, node));
                    match waiter {
                        Some(waiter) => {
                            let _ = waiter.send(Frame {
                                message: Message::Reply { status, data },
                                fds: frame.fds,
                            });
                        }
                        None => log::debug!(
                            "client {} answered node {} with nobody waiting",
                            client,
                            node
                        ),
                    }
                }

                Message::Acquire { handle } => {
                    self.broker.lock().unwrap().acquire(client, handle)?;
                }
                Message::Release { handle } => {
                    let released = self.broker.lock().unwrap().release(client, handle)?;
                    log::trace!(
                        "client {} released handle {} ({:?})",
                        client,
                        handle,
                        released
                    );
                }
                Message::IncRefs { handle } => {
                    self.broker.lock().unwrap().inc_refs(client, handle)?;
                }
                Message::DecRefs { handle } => {
                    self.broker.lock().unwrap().dec_refs(client, handle)?;
                }

                Message::LinkToDeath { handle } => {
                    self.broker.lock().unwrap().link_to_death(client, handle)?;
                }
                Message::UnlinkToDeath { handle } => {
                    self.broker
                        .lock()
                        .unwrap()
                        .unlink_to_death(client, handle)?;
                }

                Message::Export { name, node } => {
                    match self.broker.lock().unwrap().export(client, &name, node) {
                        Ok(()) => {
                            log::debug!("client {} exported {} as node {}", client, name, node)
                        }
                        Err(e) => log::warn!("client {} could not export {}: {}", client, name, e),
                    }
                }

                Message::Lookup { name } => {
                    let found = self.broker.lock().unwrap().lookup(client, &name);
                    let answer = match found {
                        Some((handle, node, owner)) => Message::Found {
                            handle,
                            node,
                            owner,
                        },
                        None => Message::Found {
                            handle: NO_HANDLE,
                            node: 0,
                            owner: 0,
                        },
                    };
                    self.to_peer(client, answer, &[])?;
                }

                other => anyhow::bail!("a client may not send {:?}", other),
            }
        }
    }

    fn transaction(&self, client: ClientId, call: Call) -> anyhow::Result<()> {
        let dispatch = self.broker.lock().unwrap().transact(
            client,
            call.handle,
            call.code,
            call.flags,
            call.data.clone(),
        )?;

        match dispatch {
            Dispatch::Reply { status, data } => {
                if call.flags & TF_ONE_WAY == 0 {
                    self.to_peer(client, Message::Reply { status, data }, &[])?;
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
                            status: -1,
                            data: b"a local object is called directly".to_vec(),
                        },
                        &[],
                    )?;
                }
            }
            Dispatch::Forward { owner, node, .. } => self.forward(client, owner, node, call)?,
        }
        Ok(())
    }

    /// Ask the process that owns a node to run a transaction, and relay the
    /// answer to whoever asked.
    fn forward(
        &self,
        client: ClientId,
        owner: ClientId,
        node: u64,
        call: Call,
    ) -> anyhow::Result<()> {
        let (sender, receiver) = sync_channel(1);
        self.pending.lock().unwrap().insert((owner, node), sender);
        let descriptors: Vec<RawFd> = call.fds.iter().map(|fd| fd.as_raw_fd()).collect();
        self.to_peer(
            owner,
            Message::Incoming {
                from: client,
                node,
                code: call.code,
                flags: call.flags,
                data: call.data,
            },
            &descriptors,
        )?;

        if call.flags & TF_ONE_WAY != 0 {
            self.pending.lock().unwrap().remove(&(owner, node));
            return Ok(());
        }

        let answer = receiver.recv_timeout(FORWARD_TIMEOUT);
        self.pending.lock().unwrap().remove(&(owner, node));
        let answer = answer
            .map_err(|_| anyhow::anyhow!("no answer for node {} from client {}", node, owner))?;
        let fds: Vec<RawFd> = answer.fds.iter().map(|fd| fd.as_raw_fd()).collect();
        self.to_peer(client, answer.message, &fds)
    }

    fn to_peer(&self, client: ClientId, message: Message, fds: &[RawFd]) -> anyhow::Result<()> {
        let peer = self.peers.lock().unwrap().get(&client).cloned();
        let peer = peer.ok_or_else(|| anyhow::anyhow!("no connection for client {}", client))?;
        // The lock on the writer is taken after the map's is dropped, so a slow
        // connection cannot hold up everyone else's routing.
        let sent = peer.writer.lock().unwrap().send(&message, fds);
        sent
    }
}

/// The pid of the process on the other end, from the kernel rather than from
/// anything the client says about itself.
fn peer_pid(fd: RawFd) -> anyhow::Result<u32> {
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
    use std::os::unix::net::{UnixListener, UnixStream};

    struct Echo;

    impl BinderObject for Echo {
        fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Vec<u8>> {
            Ok([b"echo:".as_slice(), &code.to_be_bytes(), data].concat())
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

        let acceptor = transport.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
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

    fn own_node(counter: u32) -> u64 {
        ((std::process::id() as u64) << 32) | counter as u64
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
                handle,
                code: 7,
                flags: 0,
                data: b"parcel".to_vec(),
            },
            &[],
        )
        .unwrap();
        match conn.recv().unwrap().message {
            Message::Reply { status, data } => {
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
        let node = own_node(42);
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
                    handle,
                    code: 3,
                    flags: 0,
                    data: b"who is it".to_vec(),
                },
                &[],
            )
            .unwrap();

        let from = match h.owner.recv().unwrap().message {
            Message::Incoming {
                from,
                node: asked,
                code,
                data,
                ..
            } => {
                assert_eq!(asked, node);
                assert_eq!(code, 3);
                assert_eq!(data, b"who is it");
                from
            }
            other => panic!("expected Incoming, got {:?}", other),
        };
        assert!(from > 0, "the caller is named so the answer can be routed");

        h.owner
            .send(
                &Message::IncomingReply {
                    node,
                    status: 0,
                    data: b"the owner".to_vec(),
                },
                &[],
            )
            .unwrap();

        match h.caller.recv().unwrap().message {
            Message::Reply { status, data } => {
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
        let node = own_node(77);
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
                    handle,
                    code: 1,
                    flags: 0,
                    data: Vec::new(),
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
                    node,
                    status: 0,
                    data: Vec::new(),
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
        let node = own_node(99);
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
                    handle,
                    code: 1,
                    flags: 0,
                    data: b"register".to_vec(),
                },
                &[],
            )
            .unwrap();
        match h.caller.recv().unwrap().message {
            Message::Reply { status, data } => {
                assert_eq!(status, 0);
                assert!(data.starts_with(b"echo:"));
            }
            other => panic!("expected Reply, got {:?}", other),
        }
    }
}
