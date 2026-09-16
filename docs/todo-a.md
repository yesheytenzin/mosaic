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
- [x] The Java path reaches it. The request layout was the thing in the way: the
      interface token has a twelve-byte prefix and an int32 between it and the
      name, so the name parsed as empty and every service looked absent. Names
      are read correctly now, registrations are remembered, and lookups find them
      (`checkService memtrack.proxy found`).
- [x] Hold a reference to a remembered object. It is taken by the reader that
      takes the object -- `Parcel::readStrongBinder` -- and deliberately not
      released, so the registry owns the service from registration on. The hand
      parse of the flat_binder_object, which stored a bare pointer and let it go
      stale under the framework's `getService`, is gone. Verified: no SIGSEGV, no
      staleness guard, and lookups still find what is registered.
- [~] A transaction for a *handle*, the shim forwarding to the broker, and the
      shim as a client of the transport. Written and building: connect, `Export`
      on registration, `Lookup` on a local miss, `Transaction` for an unknown
      handle, a reader thread that serves an `Incoming` by building a Parcel over
      the bytes and entering `BBinder::transact`, and `tools/two-process-call.py`
      as the second process. **Not verified, and off by default.** The first run
      published nothing and the reason is not yet found; since a local miss would
      then wait on a socket for every service the framework does not have, it
      stays off behind `MOSAIC_BINDER_BROKER=1` rather than turned on untested.

*Gate:* a service registered by name is found by name and a transaction reaches
it. Met by the broker's own tests, by `tools/binder-probe.py` against the shipped
daemon, and now by the framework itself: `AServiceManager_addService` and
`ServiceManager.addService` both land, and `checkService` finds what they
registered.

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

The binder path is answered. The system server now stops on something that is not
a binder problem: it waits for `installd`, a native daemon on a device, and

```
Installer: installd not found; trying again
```

repeats until the run ends. The service manager is telling the truth -- there is no
installd -- and the fix is for one to exist. Two ways, and they are the same work
seen from two sides: implement the AIDL service in Rust and host it in the broker,
and have the shim forward a lookup it cannot answer to the broker over the socket.
That is what `src/binder/transport.rs` was built for, and it is the next item.
