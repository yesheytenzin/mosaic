# A: the critical path, as a checklist

The items in `docs/remaining-work.md` section A, in dependency order, each with the
gate that says it is done. Kept here because it is the list being worked.

## A1. Font map ✅

Done. The shim redirected `open` but not `stat`/`access`, and the font parser
filters by `File.exists`.

## A2. The service registry ✅

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
- [x] A transaction for a *handle*, the shim forwarding to the broker, and the
      shim as a client of the transport. Verified from both ends, with the
      framework in one process and `tools/two-process-call.py` in another:

      - the shim publishes what it registers -- `published memtrack.proxy`
      - a second process resolves a name the first published -- `handle=1
        node=0xe53800000002 owner=1`
      - the broker forwards the call to the owner -- `the broker sent a
        transaction`
      - the owner serves it and answers -- `served node ...` on one side, and a
        Reply with status 0 on the other

      The hang in the way of the last of those was the request Parcel:
      `ipcSetDataReference` never returned, so the call was never dispatched and
      the caller waited until the broker gave up. The bytes the sender wrote are
      now copied into a fresh Parcel instead. On by default; `MOSAIC_BINDER_BROKER=0`
      turns it off for a harness with no broker to talk to.

      A binder object in an *answer* is carried now, and that is a different
      thing from one in the arguments. `Answer` on the Rust side names objects
      inside its data (`ObjectRef`/`Handed`), the transport sees a node and the
      caller's table and writes the caller's handle where the object word is, the
      wire appends the offsets after the data, and the shim writes each one
      through libbinder's own object writer so that it lands in the caller's
      object table -- an object missing from that table is refused rather than
      handed over. `ISystemSuspend.acquireWakeLock` is the first user: it used to
      answer with a null object, and the framework's `disableAutoSuspend` asserts
      the lock it asked for is not null.

      The *argument* direction is carried now too, and it is the mirror: a
      caller passing the binder it owns into a transaction. The sender's shim
      finds the objects in the request parcel (from libbinder's own object table,
      which is the only thing that knows where they are -- the bytes alone cannot
      say), exports a local one as an unnamed node and leaves a handle one alone
      with a node of zero; the broker resolves each into a node, checks that the
      sender is entitled to it, and writes the *callee's* handle into the object
      word before forwarding; the callee's shim writes those objects through
      libbinder's writer as it rebuilds the request parcel, so the callee's
      object table holds them. The node rides in the object word's cookie, which
      is what lets a process handed its own object back recognize it and get the
      local binder rather than a proxy that would call itself through the broker.

      Verified across three processes: `tools/two-process-argument.py` passes the
      suspend hal's handle to another process's object, that process calls it
      back, and what the hal said comes home (`tools/two-process-owner.py` does
      the calling). Both directions are in the gate.

      The sequence, to reproduce it by hand:

      ```
      MOSAIC_SOCKET=/tmp/mosaic-broker.sock target/debug/mosaic daemon &
      cd <bundle> && MOSAIC_BINDER_BROKER=1 MOSAIC_BINDER_SOCKET=/tmp/mosaic-broker.sock \
        MOSAIC_ANDROID_ROOT=$PWD MOSAIC_PROPERTY_DIR=$PWD/properties MOSAIC_TIMEOUT=150 \
        MOSAIC_PRELOAD="<repo>/tools/launcher/out/launcher.so \
          <repo>/tools/binder-shim/out/{probe,pretend-nice,android-binder,android-properties}.so" \
        MOSAIC_LAUNCH_CLASS=com.android.server.SystemServer \
        MOSAIC_LAUNCH_RUNTIME=$PWD/lib64/libandroid_runtime.so \
        <repo>/tools/bundle/with-logd.sh $PWD/run.sh dalvikvm64 \
          -Xbootclasspath:"$(cat bootclasspath.txt)" -cp "$(cat systemserverclasspath.txt)"
      # in another shell, once the broker's log says a name was exported:
      tools/two-process-call.py /tmp/mosaic-broker.sock <that name> 1
      ```
      `tools/verify-two-process-call.sh` runs the manual sequence end to end. It
      builds the shim and native artifacts first, uses a persistent owner for the
      forwarding assertion, and exits nonzero if the owner does not publish or
      answer. The framework's own service remains best-effort because that
      process can exit during boot; the persistent owner is the gate for
      forwarding.

*The defect, with the measurements — fixed, and kept for the record.* The
framework's own services did not reach the registry, so a second process could not
reach one. It was not the request format and not an empty object table; three Java
`addService` requests, dumped as they arrive:

| service | request size | name ends at | object table | first offset |
| --- | --- | --- | --- | --- |
| `system_server_dumper` | 156 | 116 | 1 entry | 120 |
| `platform_compat` | 144 | 108 | 1 entry | 108 |
| `platform_compat_native` | 160 | 120 | 1 entry | 124 |

and the bytes around the object, from two of them:

```
6d 00 70 00 65 00 72 00 | 00 00 00 00 | 85 2a 62 73 | 00 01 00 00 | 70 46 a5 17 b7 7a 00 00 | b0 da a0 47 b7 7a 00 00
   "...mper"               four bytes     TYPE=BINDER   flags=0x100    binder=weakrefs             cookie=the object

70 00 61 00 74 00 00 00 |              85 2a 62 73 | 00 01 00 00 | 20 e6 a6 17 b7 7a 00 00 | 10 c0 a0 47 b7 7a 00 00
   "...pat"                             TYPE=BINDER   flags=0x100    binder=weakrefs             cookie=the object
```

What that settles:

- the object is where the format says it is -- `type @ 0`, `flags @ 4`, `binder @ 8`,
  `cookie @ 16`, 24 bytes -- and the cookie is the IBinder, as
  `Parcel::unflattenBinder`'s own code has it (`mov 0x10(%rbx),%r12` for the BINDER
  case)
- the padding after a name is **not** a function of its length alone: 20 characters
  are followed by four bytes, 15 by none, and neither "pad to four" nor "pad to
  eight" explains both
- the request contains the object **twice**: searching for the type word finds a
  match at 108 and another at 120 in the same request, reporting the *same* cookie

The failed experiments are kept as history, not as open work. The live service
registry now uses libbinder's reader (`readStrongBinder`) and the broker's
per-process handle table. A name registered by the Java or AIDL path is found by
name, and a transaction to it is routed and answered. The gate below is the
current verification; the old pointer-search experiment is not a fallback.

*Gate:* a service registered by name is found by name and a transaction reaches
it. **Met.** Both halves:

- The **AIDL** path: `AServiceManager_addService` registrations land and are found
  (`memtrack.proxy`, `android.frameworks.stats.IStats/default`), and the object is
  now *kept alive* — `AIBinder_incStrong` at registration, never released, the way
  the Java door's `readStrongBinder` leaves a reference behind. Without it the
  caller's reference went away with the caller and the registry answered a stale
  pointer as absent (one `no longer looks like an object` line per boot).
- The **Java** path: `activity_task`, `platform_compat`, `file_integrity`,
  `uri_grants`, `powerstats` and the rest of `ServiceManager.addService` land, are
  published to the broker, and are handed back — verified from a boot log, and from
  a second process calling one of them (`tools/verify-two-process-call.sh`).

The measurements above are kept because they are what the bug *looked* like; the
resolution is in `docs/remaining-work.md` under "Where the system server is now".

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

## A5. The broker transport ✅

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
- [x] The C shim as a client of it -- connect, `Export` on registration, `Lookup`
      on a local miss, `Transaction` for a handle it does not own, and a reader
      thread that serves an `Incoming` by entering `BBinder::transact` on the
      local object. Verified from both ends against a running broker.

*Gate:* a transaction between two processes works. Met over real sockets by
`a_transaction_crosses_between_two_connections`, and end to end by the framework's
own process serving a call from a second one. `tools/verify-two-process-call.sh`
also checks reply objects, argument objects, descriptors, and the display-half
service against a real runtime bundle.

## A6. One privileged step ✅ (gate verified)

- [x] `LimitNICE` on the broker's user unit
- [x] The same grant system side (`system/user@.service.d/mosaic.conf`), because a
      user service cannot raise its own hard limit -- without this the unit's line
      is silently a no-op
- [x] `tmpfiles.d` entry for the paths Bionic compiles in and cannot be
      redirected
- [x] A test that the two limits agree, so a drift cannot silently reintroduce the
      failure
- [x] The install carries it: a staged install (`make install DESTDIR=...`, no
      root) lands all fourteen files, including
      `usr/lib/systemd/system/user@.service.d/mosaic.conf` (the grant) and
      `usr/lib/systemd/user/mosaic-broker.{service,socket}` (the unit that repeats
      it). The drift test passes: both say `LimitNICE=40`.
- [x] Re-checked as the user, which is where the failure would show: the grant is
      in `/usr/lib/systemd/system/user@.service.d/mosaic.conf`, the broker's unit
      repeats it, `systemctl --user show mosaic-broker.service -p LimitNICE`
      reports `40` for the *running* manager, and a transient unit with the limit
      lowers its niceness to `-20` while the same unit with `LimitNICE=0` is
      refused (`[Errno 13] Permission denied`). The last check is the control the
      first three need: they say the setting is there, it says the setting works.
- [x] Verify without the harness stand-ins, **as a user**. `make verify-priority`
      was the one command that needed root, because it raised the limit itself with
      `ulimit`/`setpriv`. But on an installed machine the limit is *already* in
      effect -- `LimitNICE` on the user manager reaches every process in the session
      -- so raising it was never necessary, and requiring root made the one command
      that verifies this refuse to run exactly where it would have worked. The script
      now checks `/proc/self/limits` first and, when the limit is already there, runs
      the framework directly. As this user:

      ```
      3. what the user manager reports
        ok    LimitNICE=40
      4. can a child of the broker lower its niceness?
        ok    Max nice 40 in effect for this process, so nothing has to be raised
      5. the framework, without pretend-nice.so
        ok    reached StartActivityManager with no priority stand-ins
      everything is in place: the session's limit, and the framework running
      without the priority stand-ins.
      ```

      The root path is still there for a machine where the limit is not yet in
      effect, and it says which of the two to do.
- [x] Verify without the harness stand-ins. **Verified.** With `LimitNICE=40`
      granted for one run -- `sudo tools/verify-priority-limit.sh <bundle>`, which
      raises it and hands the process to the invoking user with `setpriv` -- and
      `pretend-nice.so` left out of the preload list:

      ```
      5. the framework, without pretend-nice.so (limit raised for this run)
        ok    reached StartActivityManager with no priority stand-ins
      ```

      The same run without the limit, and without the stand-ins, stops at
      `InitBeforeStartServices` with `SecurityException` from `setThreadPriority`.
      So the limit is what the stand-ins were standing in for.

      Two things learned on the way, both worth keeping:

      - The stand-ins were doing **two unrelated jobs**, and the first run with the
        whole file removed failed for the *other* one:

        ```
        java.lang.SecurityException: No permission to modify given thread 27270
          at android.os.Process.setThreadGroup(Native Method)
          at com.android.server.UiThread.run(UiThread.java:44)
        ```

        That is `set_sched_policy` and `TaskProfiles::SetTaskProfiles` wanting
        cgroups a desktop has not got, however high the limit is. They moved to
        `pretend-cgroups.c`, which stays: a host without cgroups needs it whatever
        the limit is, and mapping Android's scheduling groups onto one is work the
        product still owes.
      - `RLIMIT_NICE` cannot be raised from a user namespace, so the session-wide
        half of this needs the package installed and a new session -- which is what
        checks 1-4 of the same script are for.

      `pretend-nice.c` stays in the tree for harness runs on a machine whose
      session has no limit. The product does not need it, which is the point.


## A7. Path breadth ✅

Done: `/vendor`, `/product`, `/system_ext`, `/odm`, and the vendor library list.

## A8. Properties that do not exist yet ✅

Done: the trie has a catch-all prefix and the write path is in the shim. The
failing property was 34 characters against libc's 32-character limit.

## A9. `SurfaceFlinger`'s display half ✅

`src/device/surfaceflinger.rs` is hosted under the names the framework looks up.
It reads this host's `/sys/class/drm`, distinguishes the legacy and AIDL
interfaces by token, and returns the display ids, mode, density, state, token,
and display-event channel expected by the framework. The compositing half
(`createConnection`, vsync, surfaces) is not part of this item and remains in
section D.

## A status

All items in the A critical path are implemented and their gates pass. The
single repeatable gate is:

```
tools/verify-a.sh <runtime-bundle>
```

It runs the Rust checks, warning-free native builds, the live Binder/display
gate, the installed `LimitNICE` gate, and the real `SystemServer` smoke. The
smoke accepts a later B failure only after proving the A milestones: font
loading, service registration, display construction, path redirection, and
property reads/writes.

The current system-server crash after those A gates is a later B item, not an
unfinished A gate. The compositing half of the windowing phase is section D.
