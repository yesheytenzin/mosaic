// SPDX-License-Identifier: GPL-3.0-or-later

//! The Binder data plane: frames on a Unix socket, carrying descriptors (A4).
//!
//! Separate from `broker::protocol`, which is JSON for the control plane and
//! carries no descriptors. A connection says which it is in its first four
//! bytes: the magic below, or a length prefix. The magic read as a length is
//! far past [`MAX_FRAME`], so the two cannot be mistaken for each other.
//!
//! The header is fixed size and big-endian, because the C shim writes it too.

use nix::sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags};
use std::collections::VecDeque;
use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// The first four bytes on a Binder connection.
pub const MAGIC: [u8; 4] = *b"MSBD";
/// Refuse absurd frames rather than allocating on a hostile length.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
/// As many descriptors as one transaction may carry.
pub const MAX_FDS: usize = 32;
/// The transaction is oneway and owes no reply.
pub const TF_ONE_WAY: u32 = 0x01;

const HEADER: usize = 32;
const VERSION: u8 = 1;

const KIND_TRANSACTION: u8 = 0;
const KIND_REPLY: u8 = 1;
const KIND_ACQUIRE: u8 = 2;
const KIND_RELEASE: u8 = 3;
const KIND_INCREFS: u8 = 4;
const KIND_DECREFS: u8 = 5;
const KIND_DEAD: u8 = 6;
const KIND_INCOMING: u8 = 7;
const KIND_BYE: u8 = 8;
const KIND_EXPORT: u8 = 9;
const KIND_LOOKUP: u8 = 10;
const KIND_FOUND: u8 = 11;
const KIND_INCOMING_REPLY: u8 = 12;
const KIND_LINK: u8 = 13;
const KIND_UNLINK: u8 = 14;

/// A handle that names nothing, for [`Message::Found`].
pub const NO_HANDLE: u32 = u32::MAX;

/// One message on the data plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A transaction. The handle is in the sender's table.
    Transaction {
        handle: u32,
        code: u32,
        flags: u32,
        data: Vec<u8>,
    },
    /// The answer to a transaction. `status` is negative on failure, as
    /// Binder's own statuses are.
    Reply { status: i32, data: Vec<u8> },
    /// A strong reference taken.
    Acquire { handle: u32 },
    /// A strong reference dropped.
    Release { handle: u32 },
    /// A weak reference taken.
    IncRefs { handle: u32 },
    /// A weak reference dropped.
    DecRefs { handle: u32 },
    /// A service this process was using has died.
    Dead { handle: u32 },
    /// The broker asking the owner of a node to run a transaction. The owner
    /// is named by `from`, so its answer can be routed back.
    Incoming {
        from: u32,
        node: u64,
        code: u32,
        flags: u32,
        data: Vec<u8>,
    },
    /// The answer to an [`Message::Incoming`]. Separate from `Reply` on
    /// purpose: a process can be asked to run one while it is waiting for an
    /// answer of its own, and the two must not be confused.
    IncomingReply {
        node: u64,
        status: i32,
        data: Vec<u8>,
    },
    /// Publish a name for a node this process owns.
    Export { name: String, node: u64 },
    /// Ask for the handle of a name.
    Lookup { name: String },
    /// The answer to a [`Message::Lookup`]. `NO_HANDLE` means no such name.
    Found { handle: u32, node: u64, owner: u32 },
    /// Ask to be told when the owner of a handle goes away.
    LinkToDeath { handle: u32 },
    /// Stop asking.
    UnlinkToDeath { handle: u32 },
    /// The session is over.
    Bye,
}

impl Message {
    fn kind(&self) -> u8 {
        match self {
            Message::Transaction { .. } => KIND_TRANSACTION,
            Message::Reply { .. } => KIND_REPLY,
            Message::Acquire { .. } => KIND_ACQUIRE,
            Message::Release { .. } => KIND_RELEASE,
            Message::IncRefs { .. } => KIND_INCREFS,
            Message::DecRefs { .. } => KIND_DECREFS,
            Message::Dead { .. } => KIND_DEAD,
            Message::Incoming { .. } => KIND_INCOMING,
            Message::IncomingReply { .. } => KIND_INCOMING_REPLY,
            Message::Export { .. } => KIND_EXPORT,
            Message::Lookup { .. } => KIND_LOOKUP,
            Message::Found { .. } => KIND_FOUND,
            Message::LinkToDeath { .. } => KIND_LINK,
            Message::UnlinkToDeath { .. } => KIND_UNLINK,
            Message::Bye => KIND_BYE,
        }
    }

    /// `(a, b, c, node, data)`, the shape the header carries.
    fn fields(&self) -> (u32, u32, u32, u64, &[u8]) {
        match self {
            Message::Transaction {
                handle,
                code,
                flags,
                data,
            } => (*handle, *code, *flags, 0, data),
            Message::Reply { status, data } => (*status as u32, 0, 0, 0, data),
            Message::Acquire { handle }
            | Message::Release { handle }
            | Message::IncRefs { handle }
            | Message::DecRefs { handle }
            | Message::Dead { handle }
            | Message::LinkToDeath { handle }
            | Message::UnlinkToDeath { handle } => (*handle, 0, 0, 0, &[]),
            Message::Incoming {
                from,
                node,
                code,
                flags,
                data,
            } => (*from, *code, *flags, *node, data),
            Message::IncomingReply { node, status, data } => (*status as u32, 0, 0, *node, data),
            Message::Export { name, node } => (0, 0, 0, *node, name.as_bytes()),
            Message::Lookup { name } => (0, 0, 0, 0, name.as_bytes()),
            Message::Found {
                handle,
                node,
                owner,
            } => (*handle, *owner, 0, *node, &[]),
            Message::Bye => (0, 0, 0, 0, &[]),
        }
    }

    /// The bytes for this message, given how many descriptors accompany it.
    pub fn encode(&self, fd_count: usize) -> Vec<u8> {
        let (a, b, c, node, data) = self.fields();
        let mut out = Vec::with_capacity(HEADER + data.len());
        out.push(self.kind());
        out.push(VERSION);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&a.to_be_bytes());
        out.extend_from_slice(&b.to_be_bytes());
        out.extend_from_slice(&c.to_be_bytes());
        out.extend_from_slice(&node.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(&(fd_count as u32).to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    /// How many descriptors the frame claims, from its header alone.
    pub fn fd_count(bytes: &[u8]) -> Option<usize> {
        if bytes.len() < HEADER {
            return None;
        }
        Some(u32::from_be_bytes(bytes[28..32].try_into().ok()?) as usize)
    }

    /// Decode a whole frame. Returns the message and how many descriptors
    /// belong to it, so the caller can take exactly those.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<(Message, usize)> {
        anyhow::ensure!(bytes.len() >= HEADER, "short frame: {} bytes", bytes.len());
        anyhow::ensure!(
            bytes[1] == VERSION,
            "unsupported data plane version {}",
            bytes[1]
        );
        let a = u32::from_be_bytes(bytes[4..8].try_into()?);
        let b = u32::from_be_bytes(bytes[8..12].try_into()?);
        let c = u32::from_be_bytes(bytes[12..16].try_into()?);
        let node = u64::from_be_bytes(bytes[16..24].try_into()?);
        let data_len = u32::from_be_bytes(bytes[24..28].try_into()?) as usize;
        let fd_count = u32::from_be_bytes(bytes[28..32].try_into()?) as usize;
        anyhow::ensure!(
            bytes.len() == HEADER + data_len,
            "frame says {} data bytes, has {}",
            data_len,
            bytes.len() - HEADER
        );
        anyhow::ensure!(fd_count <= MAX_FDS, "too many descriptors: {}", fd_count);
        let data = bytes[HEADER..].to_vec();

        let message = match bytes[0] {
            KIND_TRANSACTION => Message::Transaction {
                handle: a,
                code: b,
                flags: c,
                data,
            },
            KIND_REPLY => Message::Reply {
                status: a as i32,
                data,
            },
            KIND_ACQUIRE => Message::Acquire { handle: a },
            KIND_RELEASE => Message::Release { handle: a },
            KIND_INCREFS => Message::IncRefs { handle: a },
            KIND_DECREFS => Message::DecRefs { handle: a },
            KIND_DEAD => Message::Dead { handle: a },
            KIND_INCOMING => Message::Incoming {
                from: a,
                node,
                code: b,
                flags: c,
                data,
            },
            KIND_INCOMING_REPLY => Message::IncomingReply {
                node,
                status: a as i32,
                data,
            },
            KIND_EXPORT => Message::Export {
                name: String::from_utf8(data)
                    .map_err(|_| anyhow::anyhow!("a name must be UTF-8"))?,
                node,
            },
            KIND_LOOKUP => Message::Lookup {
                name: String::from_utf8(data)
                    .map_err(|_| anyhow::anyhow!("a name must be UTF-8"))?,
            },
            KIND_FOUND => Message::Found {
                handle: a,
                node,
                owner: b,
            },
            KIND_LINK => Message::LinkToDeath { handle: a },
            KIND_UNLINK => Message::UnlinkToDeath { handle: a },
            KIND_BYE => Message::Bye,
            other => anyhow::bail!("unknown message kind {}", other),
        };
        Ok((message, fd_count))
    }
}

/// A message and the descriptors that came with it.
#[derive(Debug)]
pub struct Frame {
    pub message: Message,
    pub fds: Vec<OwnedFd>,
}

/// The reading half of a framed connection.
///
/// Reads are buffered because a stream socket has no message boundaries, and
/// descriptors arrive attached to the bytes of the `sendmsg` that carried them,
/// so they are queued in the same order.
pub struct Reader {
    fd: RawFd,
    buf: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

/// The writing half. Held behind a lock by the transport, because one process's
/// connection can be written to by a thread serving another process.
pub struct Writer {
    fd: RawFd,
}

impl Reader {
    pub fn new(fd: RawFd) -> Self {
        Self {
            fd,
            buf: Vec::new(),
            fds: VecDeque::new(),
        }
    }

    pub fn recv(&mut self) -> anyhow::Result<Frame> {
        loop {
            if let Some((message, want)) = self.take()? {
                anyhow::ensure!(
                    self.fds.len() >= want,
                    "frame claims {} descriptors, {} arrived",
                    want,
                    self.fds.len()
                );
                let fds = self.fds.drain(..want).collect();
                return Ok(Frame { message, fds });
            }
            self.fill()?;
        }
    }

    /// A complete frame in the buffer, if there is one.
    fn take(&mut self) -> anyhow::Result<Option<(Message, usize)>> {
        if self.buf.len() < HEADER {
            return Ok(None);
        }
        let data_len = u32::from_be_bytes(self.buf[24..28].try_into()?) as usize;
        anyhow::ensure!(data_len <= MAX_FRAME, "frame too large: {} bytes", data_len);
        let total = HEADER + data_len;
        if self.buf.len() < total {
            return Ok(None);
        }
        let frame: Vec<u8> = self.buf.drain(..total).collect();
        let (message, fds) = Message::decode(&frame)?;
        Ok(Some((message, fds)))
    }

    fn fill(&mut self) -> anyhow::Result<()> {
        let mut chunk = vec![0u8; 64 * 1024];
        // Room for MAX_FDS descriptors, which is four bytes each plus a header.
        let mut control = vec![0u8; 1024];
        let mut received: Vec<RawFd> = Vec::new();
        let bytes;
        {
            let mut iov = [IoSliceMut::new(&mut chunk)];
            let msg = recvmsg::<()>(self.fd, &mut iov, Some(&mut control), MsgFlags::empty())?;
            bytes = msg.bytes;
            for cmsg in msg.cmsgs() {
                if let ControlMessageOwned::ScmRights(descriptors) = cmsg {
                    received.extend(descriptors);
                }
            }
        }
        if bytes == 0 {
            anyhow::bail!("the peer closed the connection");
        }
        self.buf.extend_from_slice(&chunk[..bytes]);
        anyhow::ensure!(
            self.fds.len() + received.len() <= MAX_FDS,
            "too many descriptors in flight"
        );
        for fd in received {
            // SAFETY: the kernel put this descriptor in our table and nothing
            // else has taken ownership of it.
            self.fds.push_back(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        Ok(())
    }
}

impl Writer {
    pub fn new(fd: RawFd) -> Self {
        Self { fd }
    }

    pub fn send(&mut self, message: &Message, fds: &[RawFd]) -> anyhow::Result<()> {
        anyhow::ensure!(fds.len() <= MAX_FDS, "too many descriptors: {}", fds.len());
        let bytes = message.encode(fds.len());
        let iov = [IoSlice::new(&bytes)];
        let rights = [ControlMessage::ScmRights(fds)];
        let cmsgs: &[ControlMessage] = if fds.is_empty() { &[] } else { &rights };
        let sent = sendmsg(self.fd, &iov, cmsgs, MsgFlags::empty(), None::<&()>)?;
        anyhow::ensure!(
            sent == bytes.len(),
            "short send: {} of {} bytes",
            sent,
            bytes.len()
        );
        Ok(())
    }
}

/// Both halves of a connection a caller owns outright.
pub struct Conn {
    reader: Reader,
    writer: Writer,
    fd: RawFd,
}

impl Conn {
    pub fn new(fd: RawFd) -> Self {
        Self {
            reader: Reader::new(fd),
            writer: Writer::new(fd),
            fd,
        }
    }

    pub fn send(&mut self, message: &Message, fds: &[RawFd]) -> anyhow::Result<()> {
        self.writer.send(message, fds)
    }

    pub fn recv(&mut self) -> anyhow::Result<Frame> {
        self.reader.recv()
    }

    /// Take the two halves apart, so one thread can read while another writes.
    pub fn split(self) -> (Reader, Writer) {
        (self.reader, self.writer)
    }

    /// The raw descriptor, for a caller that needs `poll` or `getsockopt`.
    pub fn raw_fd(&self) -> RawFd {
        self.fd
    }
}

/// Whether the four byte prefix this connection sent is the Binder magic.
pub fn is_binder_prefix(prefix: &[u8; 4]) -> bool {
    prefix == &MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::socket::{socketpair, AddressFamily, SockFlag, SockType};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;

    fn pair() -> (OwnedFd, OwnedFd) {
        socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .unwrap()
    }

    #[test]
    fn a_transaction_roundtrips() {
        let (a, b) = pair();
        let mut client = Conn::new(a.as_raw_fd());
        let mut server = Conn::new(b.as_raw_fd());

        let sent = Message::Transaction {
            handle: 12,
            code: 3,
            flags: 0,
            data: b"parcel".to_vec(),
        };
        client.send(&sent, &[]).unwrap();

        let frame = server.recv().unwrap();
        assert_eq!(frame.message, sent);
        assert!(frame.fds.is_empty());
    }

    /// A4's gate: a descriptor sent in one process is usable in the other.
    #[test]
    fn a_descriptor_comes_through() {
        let (a, b) = pair();
        let mut client = Conn::new(a.as_raw_fd());
        let mut server = Conn::new(b.as_raw_fd());

        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"payload").unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let sent = Message::Transaction {
            handle: 1,
            code: 9,
            flags: 0,
            data: Vec::new(),
        };
        client.send(&sent, &[file.as_raw_fd()]).unwrap();

        let frame = server.recv().unwrap();
        assert_eq!(frame.message, sent);
        assert_eq!(frame.fds.len(), 1);

        let mut received = std::fs::File::from(frame.fds.into_iter().next().unwrap());
        let mut contents = String::new();
        received.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "payload", "the descriptor names the same file");
    }

    /// The socket is a byte stream, so a frame can arrive in pieces and the
    /// reader must not lose them.
    #[test]
    fn a_frame_split_across_writes_is_reassembled() {
        let (a, b) = pair();
        let mut server = Conn::new(b.as_raw_fd());

        let bytes = Message::Transaction {
            handle: 7,
            code: 1,
            flags: 0,
            data: b"split frame".to_vec(),
        }
        .encode(0);

        let mut writer = std::fs::File::from(a);
        writer.write_all(&bytes[..10]).unwrap();
        writer.flush().unwrap();
        writer.write_all(&bytes[10..]).unwrap();
        writer.flush().unwrap();

        let frame = server.recv().unwrap();
        assert_eq!(
            frame.message,
            Message::Transaction {
                handle: 7,
                code: 1,
                flags: 0,
                data: b"split frame".to_vec(),
            }
        );
    }

    #[test]
    fn a_hostile_length_is_refused_before_allocating() {
        let (a, b) = pair();
        let mut server = Conn::new(b.as_raw_fd());
        let mut header = vec![0u8; HEADER];
        header[0] = KIND_TRANSACTION;
        header[1] = VERSION;
        header[24..28].copy_from_slice(&((MAX_FRAME + 1) as u32).to_be_bytes());
        let mut writer = std::fs::File::from(a);
        writer.write_all(&header).unwrap();
        writer.flush().unwrap();

        assert!(server.recv().is_err());
    }

    #[test]
    fn reference_messages_roundtrip() {
        let (a, b) = pair();
        let mut client = Conn::new(a.as_raw_fd());
        let mut server = Conn::new(b.as_raw_fd());
        for message in [
            Message::Acquire { handle: 3 },
            Message::Release { handle: 3 },
            Message::IncRefs { handle: 4 },
            Message::DecRefs { handle: 4 },
            Message::Dead { handle: 4 },
            Message::Bye,
        ] {
            client.send(&message, &[]).unwrap();
            assert_eq!(server.recv().unwrap().message, message);
        }
    }

    #[test]
    fn a_negative_status_survives_the_header() {
        let reply = Message::Reply {
            status: -38,
            data: b"nope".to_vec(),
        };
        let (decoded, fds) = Message::decode(&reply.encode(0)).unwrap();
        assert_eq!(decoded, reply);
        assert_eq!(fds, 0);
    }

    #[test]
    fn the_magic_tells_the_planes_apart() {
        assert!(is_binder_prefix(&MAGIC));
        // A control plane length prefix is at most MAX_FRAME, so the magic read
        // as a length would have been refused long before this.
        assert!(!is_binder_prefix(&(4096u32.to_be_bytes())));
    }
}
