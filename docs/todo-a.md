# A: the critical path, as a checklist

The items in `docs/remaining-work.md` section A, in dependency order, each with the
gate that says it is done. Kept here because it is the list being worked.

## A1. Font map ✅

Done. The shim redirected `open` but not `stat`/`access`, and the font parser
filters by `File.exists`.

## A2. The service registry

The authority is `src/binder/broker.rs`, exercised by tests, and served over the
socket by `src/binder/transport.rs`.

- [x] A name resolves to a node and its owner, and to a handle in the asking
      process's table
- [x] Handles are per process: the owner gets its own object back, everyone else a
      proxy, and both may be the same number meaning different things
- [x] A process cannot publish a node whose id belongs to another pid
- [x] A name can only be registered once
- [x] The NDK registration pair remembers the binder (it used to be discarded, so
      `memtrack.proxy` was registered and unreachable)
- [ ] The Java path cannot reach any of it yet: `BinderProxy.transact` resolves
      inside libbinder, where no preload can interpose, so those calls go to the
      driver-level shim, whose command-stream framing is now settled but not
      implemented. See `docs/binder.md`.
- [ ] The shim forwards to the broker over the socket instead of answering locally

*Gate:* a service registered by name is found by name and a transaction reaches
it. Met by the broker's own tests and by `tools/binder-probe.py` against the
shipped daemon; not yet met for a Java caller.

## A3. Reference counting and lifetime ✅

- [x] Strong and weak counts per handle, with the entry dropped at zero
- [x] `Acquire`/`Release`/`IncRefs`/`DecRefs` on the data plane, and the broker's
      per-node accounting behind them
- [x] Death notification: `LinkToDeath`/`UnlinkToDeath`, and a `Dead` message to
      whoever asked
- [x] A process that goes away takes its nodes out of every table

*Gate:* a binder survives while referenced and is cleaned up after. Met by
`src/binder/table.rs` and `src/binder/broker.rs` tests.

## A4. Descriptor passing ✅

- [x] `SCM_RIGHTS` in the frame codec, sending and receiving
- [x] A message with a descriptor arrives with the same file
- [x] A descriptor survives a *forwarded* transaction, which is the interesting
      case: the broker hands the descriptor on rather than copying bytes
- [x] Ownership is single: received descriptors are `OwnedFd` and close when the
      last holder drops them

*Gate:* a transaction carrying an fd arrives intact. Met by three tests in
`src/binder/wire.rs` and `src/binder/transport.rs`.

## A5. The broker transport ✅ (broker side)

- [x] A framed data plane on the broker's socket, told apart from the control
      plane by its magic, which as a length prefix would be refused anyway
- [x] One thread per connection; a synchronous transaction blocks its own thread
      and nothing else
- [x] Per-process handle tables
- [x] A transaction for another process is sent to it as `Incoming` and its answer
      is relayed back to the asker
- [x] Oneway transactions do not wait
- [x] A timeout, so a process that stops answering cannot hold a caller forever
- [x] The daemon serves it: verified by `tools/binder-probe.py` against
      `mosaic daemon`, not only by tests
- [ ] The C shim as a client of it

*Gate:* a transaction between two processes works. Met over real sockets by
`a_transaction_crosses_between_two_connections`, with the two connections served
on separate threads; not yet used by the framework, because of A2's Java-path
item.

## A6. One privileged step ✅ (except one unverifiable gate)

- [x] `LimitNICE` on the broker's user unit
- [x] The same grant system side (`system/user@.service.d/mosaic.conf`), because a
      user service cannot raise its own hard limit -- without this the unit's line
      is silently a no-op
- [x] `tmpfiles.d` entry for the paths Bionic compiles in and cannot be
      redirected
- [x] A test that the two limits agree, so a drift cannot silently reintroduce the
      failure
- [ ] Verify without the harness stand-ins. Needs root: `RLIMIT_NICE` cannot be
      raised from a user namespace, so this host cannot check it.

*Gate:* `Process.setThreadPriority` works without the harness stand-ins.

## A7. Path breadth ✅

Done: `/vendor`, `/product`, `/system_ext`, `/odm`, and the vendor library list.

## A8. Properties that do not exist yet ✅

Done: the trie has a catch-all prefix and the write path is in the shim. The
failing property was 34 characters against libc's 32-character limit.

## What is left of A

One thing, and it is the reason the system server stops where it does: **answer the
Java path.** Those transactions are `BinderProxy.transact`, which resolves inside
libbinder and cannot be interposed; they land at `ioctl`, and the command stream
there is now decoded (`_IOW('c', nr, size)`, self-describing). Implementing the
reply is ordinary work: parse, dispatch `BC_TRANSACTION` for handle 0 through the
registry the shim already has at the parcel level, and write `BR_REPLY` with a
reply parcel this side allocates.
