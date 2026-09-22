# Remaining work

What is left before Mosaic runs an arbitrary Android app, in dependency order.
It is a working list: the near-term items each have a gate that can be checked by
running something, because that is how every phase so far has been settled.

Working today: the runtime bundle builds from an image in one command; ART runs a
DEX and AOT-compiles a real app; the framework starts and `SystemServer` runs
`startBootstrapServices` through `InitPowerManagement` and `StartDisplayManager`
and past the suspend path that used to kill it at the register-callback call, past
the `installd` the package manager waits for, and past the `SurfaceFlinger` the
display manager waits for: it now runs the bootstrap services and reaches the boot
phases of `startOtherServices`; properties are read and written; a userspace binder with per-process handle tables, reference
counting, descriptor passing and a broker transport is served by the daemon *and
reached by the framework*: a name another process published resolves to a handle,
a transaction on it crosses, and the owner runs it — for a Java object as well as
a native one; the device services the framework asks for by name (`suspend_control`,
`suspend_control_internal`, the `ISystemSuspend` HAL) are hosted by the broker and
answer; 151 JNI registrars resolve; the shim presents Android's absolute paths.

## A. The critical path, next

1. **Font map** ✅ — `stat`/`access` were not redirected, so the font parser could
   not see the fonts. Fixed; `SystemServer` moved on.
2. **Binder registry.** ✅ — the authority is `src/binder/broker.rs`, served over
   the socket by `src/binder/transport.rs`, and it is exercised: a name resolves to a
   node and an owner, handles are per process, a transaction to another process is
   forwarded and its answer relayed, and a process cannot publish a node that
   belongs to another pid. What is *not* done is the shim forwarding to it, and
   behind that a harder fact: the Java path cannot reach any of it, because
   `BinderProxy.transact` resolves inside libbinder and no preload can see it.
   Those calls land at `ioctl`, whose framing is settled (see `docs/binder.md`) and
   whose reply is written: the shim forwards, the broker answers, and the Java path
   reaches both. What is carried now includes objects in answers and in arguments
   and descriptors in both directions. *Gate:* met -- a service registered by name is
   found by name, a transaction reaches it, and `tools/verify-two-process-call.sh`
   proves it with an owner that stays up.
3. **Reference counting and lifetime** ✅ — `src/binder/table.rs` holds the counts
   and `src/binder/broker.rs` the accounts, with `Acquire`/`Release`/`IncRefs`/
   `DecRefs` and `LinkToDeath`/`UnlinkToDeath` on the wire. A process that goes
   away takes its nodes out of every table and tells whoever asked. *Gate:* met.
4. **Descriptor passing** ✅ — `SCM_RIGHTS` in the codec, and a descriptor
   survives a forwarded transaction, which is the case that matters: the broker
   hands it on rather than copying bytes. *Gate:* met.
5. **Broker transport** ✅ — the data plane is a framed, self-describing protocol
   on the broker's own socket, told apart from the control plane by its magic; one
   thread per connection; per-process handle tables; a transaction for another
   process is sent as `Incoming` and the answer relayed. `tools/binder-probe.py`
   checks it against the shipped daemon, not only against tests. *Gate:* met, with
   two connections served on separate threads. What is left is the shim as a
   client of it.
6. **One privileged step (ADR-0013)** ✅ except the gate — `LimitNICE` on the
   broker's unit *and* the system-side grant to the user manager, without which
   the unit's line is silently a no-op, plus a `tmpfiles.d` entry for the one path
   Bionic compiles in. A test keeps the two limits from drifting. *Gate:* **verified** -- with the limit raised for one run and the priority
   stand-ins left out, SystemServer still reaches `StartActivityManager`; without
   the limit and without them it stops at `InitBeforeStartServices` with a
   `SecurityException`. What remains is installing the package and starting a new
   session, which `make verify-priority` checks; the reasoning is in
   `docs/todo-a.md`.
7. **Path redirection breadth** ✅ — `/vendor`, `/product`, `/system_ext` and
   `/odm` are redirected, and the bundle carries a `vendor/etc/public.libraries.txt`
   (empty, and says so) because `SystemConfig` treats its absence as fatal.
8. **Property service growth** ✅ — the trie has a catch-all prefix, and the write
   path is implemented in the shim, which both logs every write and carries the
   ones the wire format can hold. The property that failed was
   `cache_key.is_compat_change_enabled`, 34 characters against a 32-character
   limit in libc's old-protocol check: it is accepted and not stored, which is
   what unblocked service startup.

### Where the system server is now

`startBootstrapServices` reaches `StartActivityManager` with real answers from the
service manager, and no crash:

```
SystemServerTiming: StartWatchdog ... StartInstaller ... UriGrantsManagerService
SystemServerTiming: StartPowerStatsService
SystemServerTiming: StartIStatsService          (no longer aborts)
SystemServerTiming: MemtrackProxyService
SystemServerTiming: StartActivityManager
android-binder: registered platform_compat
android-binder: checkService platform_compat found
android-binder: checkService memtrack.proxy found
```

`ActivityManagerService`'s constructor still does not complete, and the reason is
no longer a crash. Three things were fixed to get this far, each of them the
difference between a SIGSEGV and an answer:

0. **The hand-back used the wrong field.** A `flat_binder_object` has type at 0,
   flags at 4, and two pointers at 8 and 16, and `Parcel::flattenBinder()` -- the
   function on the other side -- writes `RefBase::getWeakRefs()` at 8 and the
   `BBinder` at 16 (read off its disassembly: `getWeakRefs()` result to
   `%rsp+0x8`, `rbx` to `%rsp+0x10`). The shim handed back offset 8, so
   `flattenBinder` called `localBinder()` through the weak reference table's first
   word -- its refcounts, `{strong=0, weak=1}` being `0x100000000` rather than a
   vtable. That is the `Parcel::flattenBinder+52` SIGSEGV that stopped
   `StartActivityManager`. Offset 16 is preferred now, with offset 8 only as a
   fallback, and the candidate is required to have a first word that looks like a
   vtable before it is used.
1. **The registry did not hold a reference.** `object_at` preferred
   `Parcel::readStrongBinder` -- which takes a strong reference, the way a binder
   node does -- but the call was gated on `Parcel::setDataPosition` returning
   zero, and that function returns `void`: the shim read a stale register as its
   status and skipped the reader every time. So every Java registration fell
   through to pointer arithmetic with no reference held, and a service the
   framework registers from a temporary -- `platform_compat` is one, `new
   PlatformCompat(...)` inside `startBootstrapServices` -- was freed before
   anything looked it up. With the signature corrected the reader runs: three
   registrations are now taken by `readStrongBinder` with the reference left in
   place, and `platform_compat` is still alive when it is looked up.
2. **A dead object is answered absent, not handed over.** The service manager's
   hand-back checks that the object's first word still looks like a vtable. One
   that does not is a pointer into freed memory, and passing it to `flattenBinder`
   is a SIGSEGV; answering "not found" is worse for the caller and much better
   than taking the process down.

3. **A remembered pointer that is not an IBinder is not handed back.** The NDK
   door (`AServiceManager_addService`) is given `memtrack.proxy`, and the pointer
   it hands over is not an `IBinder`: its vtable's `localBinder` slot is
   `art::MemMapArenaPool::TrimMaps`. `flattenBinder` called that as
   `localBinder()`, which is the "deadlock" it looked like from a thread dump --
   it was a wrong function, not a lock. The hand-back now checks the object
   before using it: the vtable entry at byte `0x60` has to resolve, through
   `dladdr`, into `libbinder` or `libandroid_runtime`, which is where a real
   local binder's `localBinder` lives. Anything else is answered absent.

With those there are no fatal signals at all, `StartActivityManager` completes
(194 ms), and the bootstrap services after it run: `StartDataLoaderManagerService`,
`StartIncrementalService`, `StartPowerManager`, `StartThermalManager`,
`StartHintManager`, `InitPowerManagement`. That last one used to abort on a null
`PowerManager`; it is fixed, and the boot now passes it and reaches
`StartDisplayManager`:

```
No service published for: power
java.lang.NullPointerException: newWakeLock on a null PowerManager
    at ActivityTaskSupervisor.initPowerManagement(ActivityTaskSupervisor.java:512)
```

The cause was neither the registry nor the lookup arriving. `getService` is
`ServiceManagerProxy.getService`, which calls `checkService`, so
the wire code is 2 and the trace's `smcode 2 ... found power` *is* the lookup
`getServiceOrThrow` makes: the shim is reached and answers. The reply it wrote was
correct -- an `int32` exception of 0 then a `flat_binder_object`, readable from
offset 4. What was wrong is where the reader started. The shim writes the answer
straight into the caller's Parcel, and writing leaves that Parcel's data position
at the end of what was written. A reply from a real driver arrives through
`Parcel::ipcSetDataReference`, which resets the position to zero before the
generated proxy reads it -- `readException()` and then `readStrongBinder()`. The
shim returned without that reset, so the Java reader began at byte 32 of a
32-byte parcel and every hand-back read as null. That is how `found power` and a
null `PowerManager` were both true.

The fix is one call in `handle_transaction`: `parcel_setPosition(reply, 0)` after
the answer is written, the reset a driver reply does. It is not `power`-specific;
the earlier hand-backs were reading as null too, and only the services that did
not need the object immediately tolerated it.

To see any of this, `MOSAIC_BINDER_TRACE=1` writes every registration and lookup
straight to `/tmp/mosaic-sm-trace.log` with raw syscalls, independent of the line
budget and of logd's timing. It records every `service_manager` entry with its
code and size and every `handle_transaction` entry with its code, so a lookup that
never arrives is as visible as one that does. Two things it settled, worth keeping:

- `ServiceManager.getService` maps to checkService (wire code 2), not code 1:
  AOSP-13's `ServiceManagerProxy.getService` delegates to `checkService`, and the
  old `getService` transaction is not on the path at all. A trace that shows no
  code-1 lookup for a service is therefore not evidence that the lookup did not
  happen.
- The three `answered code 7 with an empty reply` lines are
  `unregisterForNotifications` (code 7), which is not in this path.

The next wall is past `StartDisplayManager`, and it is now named exactly. The
suspend path needs three services, and all three are hosted by the broker and
answer:

| name | interface | who asks |
| --- | --- | --- |
| `suspend_control` | `android.system.suspend.ISuspendControlService` | `BatteryStatsService.nativeWaitWakeup` |
| `suspend_control_internal` | `android.system.suspend.internal.ISuspendControlServiceInternal` | `PowerManagerService`'s JNI |
| `android.system.suspend.ISystemSuspend/default` | `android.system.suspend.ISystemSuspend` | the same JNI, for the wake lock |

The lookups resolve (`getService suspend_control found in the broker, handle 17`),
the shim hands back a handle, and the framework calls one of them — and *that* call
is where it dies, inside the framework's own client code, before the transaction
reaches this side:

```
Fatal signal 11 (SEGV_MAPERR) fault addr 0x31 in tid ... (BatteryStats_wa)
  #00 android.system.suspend.control-V1-cpp.so (registerWakelockCallback+116)
  #01 android.system.suspend.control-V1-cpp.so (registerCallback+499)
  #02 libandroid_servers.so (android::nativeWaitWakeup+291)
```

`nativeWaitWakeup` calls `registerCallback` through the proxy's vtable
(`call *0x20(%rax)`, read off the disassembly), and inside it the call that crashes
is `call *0x28(%rax)` on the object at `*(r14+0x30)` — the proxy's `remote()` —
with `esi = 1`, which is `IBinder::transact(code = registerCallback)`.

The host keeps a coredump of this crash (`coredumpctl`, systemd-coredump is on by
default), and loading it in gdb settles what the disassembly could only suggest.
Unwinding past `registerWakelockCallback`'s own prologue to `registerCallback`'s
saved registers (its callee-saved `r14` is pushed before being reused, so it is
still on the stack) recovers the actual `this` of the `BpSuspendControlService`
being called: `0x7b72a5a294b0`. Its `mRemote` field, at `this+0x30`, reads
**`0x7b72a5a294b0` — the same address**. `remote()` is not a `BpBinder`; it is the
proxy's own `this`. That is exactly consistent with everything the disassembly
showed: `*(mRemote)` resolves to `vtable for BpSuspendControlService + 24` (gdb's
own symbol lookup, not a guess), and that vtable's `+0x28` slot is
`registerWakelockCallback` -- because the object being dispatched through *is* the
`BpSuspendControlService`, calling itself.

`BpRefBase(const sp<IBinder>& o) : mRemote(o.get())` cannot produce a self-pointer
during ordinary construction -- the object being constructed does not have an
address yet when `o` is captured. The only way `mRemote == this` is for `o.get()`
to already equal the address the allocator is about to hand back for `this`: the
`sp<IBinder>` `interface_cast` was given (i.e. what `readStrongBinder()` returned
for the checkService/getService reply) was carrying a pointer into memory that had
already been freed, and `new BpSuspendControlService(...)` was the allocation that
reused it. Nothing here is the shim rewriting a reply -- the flat_binder_object for
`suspend_control` is written by the same `write_handle_into` path as
`suspend_control_internal` (handle 9, works) and `ISystemSuspend/default` (handle
13, works), and `Parcel::readStrongBinder()`/`ProcessState::getStrongProxyForHandle`
on the client are unmodified libbinder. So the premature free is client-side: a
`BpBinder` (or something in the chain between it and the `BpSuspendControlService`
wrapper) that the framework still holds a reference to got destroyed early. The one
concrete difference between the service that crashes and the two that do not is
*how* it is fetched: `suspend_control` alone comes from the native
`defaultServiceManager()->waitForService()` path, called from
`BatteryStatsService.nativeWaitWakeup` on its own thread (`BatteryStats_wa`) that
had made no prior binder call in this boot; the other two go through the Java
`ServiceManager` wrapper on a thread that had already been through this machinery.
That is a lead, not a proof -- what remains is finding where in that path a
strong reference gets dropped one too many times, which wants an allocation
tracer (log every malloc/free near a `BpBinder`-sized chunk across the boot) more
than it wants another live gdb session under ART.

What is *not* in doubt is this side. The same handle mechanism works one call
later in the same boot: `suspend_control_internal` (handle 9) is called with code
3, the shim forwards it, the broker answers an empty array, and the framework
reads it without complaint (`BpBinder::transact handle 9 code 3`). The free was in
this shim's own API door, not in the framework: **the answer to a lookup was written
into the caller's Parcel, and the caller's Parcel then released the object it had
just been handed a reference to.** The mechanism, the evidence and the fix are
below, under "The allocation tracer" and "The fix"; the short version is that a
driver's reply reaches a caller as a *data reference* (which libbinder never
releases the objects of) and this one did not, so a device never has the bug.

#### The allocation tracer, and what it says

`tools/boot-system-server.sh` is the run; `tools/binder-shim/alloc-trace.c` is the
instrument. With `MOSAIC_ALLOC_TRACE=1` it logs every allocation and free inside a
size window (`MOSAIC_ALLOC_TRACE_MIN`/`MAX`, 16..256 by default) to
`/tmp/mosaic-alloc-trace.log` — one line per event with a global sequence number,
the thread, the size, the pointer, and *the address the call returns to*, because
"this address was freed" without "by whom" is not enough to fix anything. The shim
drops `handback-local`/`handback-handle` markers into the same file, so a hand-back
and the allocations around it share one sequence. The crash address is per run and
comes from the core (`coredumpctl list`, then `print/x $r15` in `frame 0`).

Run against the crash, it gives the address's whole history in one sequence, and
what it shows is *not* the shape the premature-free theory predicts:

```
alloc 63302 <main>   144 X   (ICU: utext_setup_70)
free  63304 <main>        X  (ICU: utext_close_70)
alloc 63308 <main>   144 X
...
alloc 99774 <main>   144 X   (ICU again)
free 161943 <other>       X  (ICU: utext_close_70)
handback-handle 172444 <BatteryStats> 17     ← the suspend_control lookup
alloc 172454 <BatteryStats> 152 X            ← the proxy
```

- **No `BpBinder` was ever allocated at that address in that run.** The earlier
  occupant was ICU's, freed by ICU, and the proxy took the address afterwards. So
  the self-reference in `mRemote` is not a dangling pointer the allocator handed on
  — the "the BpBinder was freed early and the proxy landed on its address"
  explanation does not fit, and the question moves to what writes `mRemote`.
- The object's layout is confirmed rather than assumed: offset 0x30 *is*
  `BpRefBase::mRemote` (`mov %rax,0x8(%rdi)` in `BpRefBase`'s constructor, with the
  `BpRefBase` subobject at +0x28 — its vtable pointer is the object's second one —
  and the generated `registerCallback` reading `0x30(%r14)` for `remote()` itself).
- One thing in the same window *is* a real defect of the premature-free kind: a
  `BINDER_TYPE_BINDER` object released by a Parcel teardown. The free's return
  address is inside `android::release_object`, and disassembling that function puts
  it in the branch `reinterpret_cast<IBinder*>(obj.cookie)->decStrong(who)` — the
  *local* binder branch, not the handle one. A local object whose `cookie` outlives
  it frees an address that may now hold something else, and the shim is the only
  writer of `cookie` in this process, so its local hand-backs are where to look.

The tracer also caught a bug in itself, which is worth knowing when reading any
trace: its first version handed blocks from a static bootstrap arena to the real
`free`, corrupting the allocator and killing the process with a bus error in an
unrelated mmap. It refuses to free those now.

All three of the wire fixes the plan named are in:

- **A correlation id on the request/answer pair.** Every lookup and transaction
  carries one, and an answer is taken only when both its kind and its id match what
  is being waited for. The broker keys its waiting answers by
  `(owner, node, id)` rather than by node alone, so two transactions to one node no
  longer collide, and the owner echoes the caller's id on its answer. This closes
  the misattribution class: an answer that arrives after its caller has given up can
  no longer be taken by the next caller as its own.
- **The timeouts agree**: the shim waits 35 s for an answer the broker is willing to
  spend 30 s producing, so giving up first can no longer turn a slow answer into a
  missing service.
- **The gate proves it.** `tools/verify-two-process-call.sh` now calls the transport
  with an owner that stays up (`tools/two-process-owner.py`) instead of relying on
  the framework, which exits a couple of seconds into its boot. The caller sends a
  distinctive request id and refuses an answer that comes back with a different one,
  and the run prints both sides: `request=0x1234` at the caller, `request 4660` at
  the owner.

Implementing the id also produced a lesson worth keeping: `broker_serve` was given
the write lock around a `broker_send` that takes the same lock, which is not
recursive. The owner then logged its serve and hung, and the broker gave up on it
thirty seconds later — visible as `no answer for node ... from client 1`, and the
reason the two-process gate failed while the same round trip through a Python owner
(a process with no lock to deadlock on) passed. Frames are now serialised in exactly
one place, inside `broker_send`.

Still open: the crash itself. Its cause is now narrower than "a premature free" —
see "What the object itself says" below — and the tracer plus the core have ruled
out the simplest reading of it.

#### What the object itself says

The core answers what the allocator log cannot. Comparing the crashing proxy
against the constructor that made it (`BpSuspendControlService::BpSuspendControlService`
in `android.system.suspend.control-V1-cpp.so`):

- Every field the constructor writes is exactly what the object holds: `+0x00` the
  interface vtable, `+0x08` `1`, `+0x10`/`+0x18` a pair of constants, `+0x20` a
  pointer to a 48-byte allocation, `+0x28` the second vptr (the `BpRefBase`
  sub-object), `+0x88` the `RefBase` virtual base. The object is **pristine**.
- The one exception is `mRemote` at `+0x30`, which holds the object's own address.
  The constructor writes it once, in `BpRefBase::BpRefBase`, as `o.get()` — the
  pointer from the `sp<IBinder>` it is handed — so the value that reached it was
  the address `operator new` had just returned.
- The allocation of that address on the crashing thread came from a caller inside
  `libstatspull.so` (`operator new` inlined there), which means a second site in
  this process also constructs a `BpSuspendControlService`, not only
  `nativeWaitWakeup`'s `getSuspendControl()`.
- `sizeof(BpSuspendControlService)` is 152: the `RefBase` virtual base at `+0x88`
  plus its 16 bytes, which is why the 152-byte allocation in the trace is the
  proxy and not something else.

So the self-reference is written by the only writer there is, from an `sp` whose
pointer was the object's own address — and the allocation history says that
address was never a `BpBinder`, so the `sp` was not a dangling pointer: it had been
handed a value that only became that address afterwards. In other words the object
it named had been *freed* and its address reused, by the proxy being constructed.

#### The fix

The over-release is in this shim's API door. (`handle_transaction`, the interposed
`IPCThreadState::transact`, is where the framework's `ServiceManager` calls arrive —
the trace shows `door 2 0` for every `checkService`.) What it did was build the
answer straight into the *caller's* `Parcel`, and libbinder treats the two shapes of
Parcel differently on destruction (`Parcel::freeDataNoInit`):

```cpp
    if (mOwner) {
        mOwner(this, mData, mDataSize, mObjects, mObjectsSize);   // a data reference
    } else {
        releaseObjects();                                         // a Parcel it owns
        ...
    }
```

A reply from a real driver arrives through `ipcSetDataReference`, which sets
`mOwner` — so libbinder never releases the objects in it; the driver's own
accounting does. Writing into the caller's Parcel, which *owns* its data, made the
caller release the object it had just read a reference to: the framework's
`BpBinder` for a service the broker hosts lost a reference it never had, was freed,
its address was reused for the ICU block and then for the proxy, and that proxy's
`mRemote` — the `sp` the lookup returned — pointed at its own address.

The answer is now built in a `Parcel` of the shim's own and handed over as a *data
reference*, with a release function that frees the buffers and releases nothing:
that is the shape a driver reply has, and it leaves the object with exactly the
reference the write took — which is what keeps it alive for the caller. The source
`Parcel` is deliberately not destroyed (its object table is what holds that
reference); it is one small allocation per service-manager call, and the only leak
this side has.

Verified: `SystemServer` now runs past the suspend path. No fatal signal, 666 log
lines instead of 398, and the services the broker hosts are being *called*:

```
--- last boot stages ---
SystemServerTiming: StartDisplayManager
--- fatal ---
power: Failed to get SystemSuspend service      ← the HAL lookup, which the boot tolerates
--- binder diagnostics ---
  0  BAD_TYPE      0  NOT_ENOUGH_DATA      3  forwarded code
```

`tools/verify-two-process-call.sh` still passes, and with it the request id
(`request=0x1234` at the caller, `request 4660` at the owner).

**The next wall is SurfaceFlinger**, and it is a *wait* rather than a crash:

```
ServiceManager: Waited one second for SurfaceFlinger (is service started? are binder threads started and available?)
```

`DisplayManagerService` wants a compositor, so the system server stops there until
one exists — that is the windowing phase below (ADR-0006, P5), and it is a subsystem
rather than a service: a Wayland-backed implementation of `Surface`,
`SurfaceControl`, `BLASTBufferQueue` and dma-buf buffers, replacing `libgui` and
SurfaceFlinger. The other wait this used to print, `Installer: installd not found;
trying again`, is gone: `installd` is item 3 above, implemented and hosted by the
broker.

2. The broker client works, and the owner serves what it is asked. Verified against
   a running broker, with the framework in one process and
   `tools/two-process-call.py` in another:

   - the shim publishes what it registers (a frame per registration, `published
     memtrack.proxy`)
   - the second process resolves a name published by the first
     (`handle=1 node=0x36e1100000001 owner=1`)
   - the broker forwards the call to the owner (`the broker sent a transaction`)
   - **the owner runs it and answers**: `served node ... code 1 failed` on one
     side, and a `Reply` carrying the owner's own status on the other

   That last line is the one that was missing. Two things were in the way, and
   both are fixed:

   - **The serving thread had no `JNIEnv`.** `JavaBBinder::onTransact` asks ART for
     the calling thread's environment and aborts when there is none
     (`JavaBinder: Binder thread started or Java binder used, but env null. Attach
     JVM?`), so a transaction from another process to a *Java* service took the
     whole framework down. The shim's reader thread now attaches to the VM through
     `JNI_GetCreatedJavaVMs` and `JavaVM::AttachCurrentThread`, which is what a
     driver's binder threads do. Verified: `serving thread attached to the Java VM`
     and `served node ...` in the owner's log, with the framework still running.
   - **Answers were taken without checking what they were.** One slot serves every
     caller, and the reader filled it with whatever arrived, so a *late* answer to
     a request that had given up was read as the next caller's: a lookup that took
     a transaction's reply read the reply's status as a handle. The awaited kind is
     now part of the state and an answer nobody waits for is dropped.
   - **A `BpBinder`'s handle was assumed to be zero.** `BpBinder::transact` reads
     it at offset 0x10 (`mov 0x10(%r12),%esi` before it calls
     `IPCThreadState::transact`, read off the disassembly), so a proxy for an object
     the broker hosts was answered as if it were the service manager. `docs/binder.md`
     called the offset unknown; it is not.

   `tools/verify-two-process-call.sh` is the gate, and it now passes: it reads the
   broker's own *log file*, because the line it waits for is written at debug level
   and the daemon's stdout carries info and above only — which is why it used to
   report that nothing was published no matter what the framework did.

3. **`installd`** ✅ — the daemon the package manager talks to, implemented in
   `src/device/installd.rs` and hosted by the broker under the name
   `Installer.connect()` looks for. It is a real implementation of the boot path,
   not a stub: `createAppData`/`createAppDataBatched` create
   `<data>/data/<package>` and `<data>/user/<user>/<package>` and return the inode,
   `getAppSize` returns six real values per package (allocated blocks, the way `du`
   counts them), `destroyAppData`/`rmdex`/`destroyAppProfiles` remove what they
   name, and `linkNativeLibraryDirectory` creates the directory the framework will
   point `nativeLibraryDir` at. The operations that exist only because Android has
   a device answer `nothing to do` honestly — `invalidateMounts` (a host
   filesystem has no storage mounts to invalidate), `restoreconAppData` (ADR-0007's
   SELinux gap, stated rather than papered over), `migrateAppData`/`fixupAppData`/
   `migrateLegacyObbData` (no older layout to move). Everything outside the boot
   path answers `EX_UNSUPPORTED_OPERATION` *with a message*, which is a refusal the
   caller can report rather than a silent lie. Ownership of the data directory is
   the app's system user's, and that is set at install time by the privileged helper
   (ADR-0008) — this side cannot do it, and does not pretend to.

   Verified: `installd not found` 184 times → **0**, `checkService installd found in
   the broker, handle 5`, and the boot carries on to the next wall.

4. **A reply that hands back an object.** Done, and it is what windowing and the
   wake lock both needed: a transaction can now answer with a binder object the
   caller has never seen. The Rust object names it by offset in its answer
   (`Answer`/`Handed`), the transport substitutes the *caller's* handle for the
   node, the wire appends the object offsets after the data so a reader that knows
   nothing of objects still reads the same bytes, and the shim writes each object
   through libbinder's own writer so it lands in the caller's object table — an
   object missing from that table is refused rather than handed over.

   The first user is `ISystemSuspend.acquireWakeLock`, which used to answer with
   the null object: the framework's `disableAutoSuspend` asserts the wake lock it
   asked for is not null, so the first caller would have crashed. It now hands
   back a real `IWakeLock` — the interface's one method, `release`, oneway, and
   counted, because nothing on this host holds the machine awake and the object
   exists to be callable rather than to hold anything.

   Verified across two processes (`tools/broker-object.py`): the hal is looked up
   by name, `acquireWakeLock` answers with a handle this process has never seen,
   and that handle answers with the *lock's* descriptor
   (`android.system.suspend.IWakeLock`) rather than the hal's.

   The mirror of it — a binder object in a transaction's *arguments*, a caller
   passing a callback it owns — is carried too: the caller's shim finds the
   objects in the request parcel from libbinder's own object table, exports a
   local one as an unnamed node (or leaves a handle alone, with a node of zero for
   the broker to resolve from the caller's table), the broker checks the sender is
   entitled to each one and writes the *callee's* handle into the object word, and
   the callee's shim registers them in its own parcel. Verified across three
   processes (`tools/two-process-argument.py`): the caller passes the suspend
   hal's handle to another process's object, that process calls it back, and what
   the hal said comes home.

5. **`SurfaceFlinger`**, the display half. ✅ — `src/device/surfaceflinger.rs`,
   hosted by the broker under the name `DisplayManagerService` waits for. It reads
   the host's own hardware (`/sys/class/drm`: the connectors that are `connected`
   and their preferred modes) and answers what the framework asks about it: which
   displays exist, what their mode and density are, whether they are on, the frame
   period, the composition color spaces, the display primaries.

   Two interfaces answer to the name `SurfaceFlinger` in Android 13 and their codes
   overlap — `bootFinished` is code 1 in the older one, `createDisplay` is code 1 in
   the AIDL one — and the framework uses *both*: `SurfaceComposerClient` reaches the
   AIDL methods for what its `.aidl` declares, and the older tags for what it does
   not (`getCompositionPreference`, `getStaticDisplayInfo`, `getDynamicDisplayInfo`).
   Which one a request is for is decided by the interface token it carries, the same
   thing `CHECK_INTERFACE` reads on a device.

   The codes and layouts are not from memory. The older ones are the tags in
   `ISurfaceComposer.h` and the write sequences in `ISurfaceComposer.cpp` from the
   branch this bundle is built from (Android 13, `TQ3A.230901.001`); the AIDL ones
   were read out of the bundle's own `libgui.so` — the immediate handed to `transact`
   in `BpSurfaceComposer::getPhysicalDisplayIds` is 3, which is that method's place in
   the AIDL declaration order. Two sources, agreeing.

   What it does *not* have is the compositing half, and it says so rather than
   pretending: `createConnection` and `createDisplayEventConnection` answer with a
   null object (no surfaces, no vsync source), a virtual display is refused by name,
   and brightness and power are the desktop's business — accepted, not obeyed.

   Getting there found a latent crash that had nothing to do with displays. A null
   display event connection makes `NativeDisplayEventReceiver::nativeInit` fail, and
   the destructor of that half-built object calls `AndroidRuntime::getJNIEnv()`,
   which dereferences `AndroidRuntime::getJavaVM()` — null, because this launcher
   creates the VM itself (the registrars have to be ordered and
   `AndroidRuntime::startReg` is not exported) and never constructs the framework's
   runtime object. The system server died with a SIGSEGV on the `android.display`
   thread. `tools/launcher/launcher.c` now publishes the VM into that object: the
   slot is found from `getJavaVM`'s own code (`mov gCurRuntime(%rip),%rax` is
   `48 8b 05` plus a displacement) rather than a recorded offset, and written only
   when the member is null. With that, the same failure is a logged
   `Failed to initialize display event receiver, status=-19` and the boot carries
   on.

   Verified: `tools/display-probe.py` (in the gate) asks the service from another
   process and checks its answers against this host's own `/sys/class/drm` — the
   display it lists, the primary, a token that round-trips as an object, the two
   info structs read back field by field, and the state that token names. The boot
   reaches the boot phases of `startOtherServices` (30 `OnBootPhase` stages, up
   from 28) with no SIGSEGV.

   **The frame clock, and descriptors on the reply path.** `DisplayManagerService`
   still times out waiting for a default display, because `LocalDisplayAdapter`
   builds its device on a `DisplayEventReceiver` and that receiver fails to
   initialize: it is the object that says *when frames happen*, and there was no
   such object. `createDisplayEventConnection` now answers with a real one, whose
   `stealReceiveChannel` hands over a live channel — a socketpair, the receive end
   to the caller and the send end kept. Nothing is written into it: the frame clock
   belongs to the host's compositor, and reaching it is the windowing phase.

   That needed descriptors on the *reply* path, which did not exist: the codec
   carried them on requests only. An answer now names a descriptor by offset
   (`Answer::handing_fd`), the broker writes a descriptor word there
   (`BINDER_TYPE_FD`, its number left for the side that can open it), the frame
   carries the descriptors themselves, and the shim writes its own numbers into the
   words — the receiving side is the only one that knows them. A descriptor arrives
   as ancillary data on the *first* byte of a frame, which is why the shim's reader
   had to become `recvmsg`.

   Verified from another process: `tools/display-probe.py` takes the connection,
   steals the channel and receives **two live descriptors** (`os.fstat` on each).

   Two things were wrong on this side of it and are fixed. The shim's reader took
   the frame *header* with `read`, and a descriptor is ancillary data on the
   *first* byte of a frame — so the descriptors were thrown away before the
   `recvmsg` that was meant to collect them, and the count check then failed the
   whole read and the reader thread gave up (after which nothing was ever answered
   again). The header is read with `recvmsg` now. And the parcelable's presence
   word was missing, which is what `status=-61` (`ENODATA`) was: the reader took the
   descriptor word's type for the flag.

   Measured on the way through: the shim reports `descriptor word at 8, 2 arrived,
   using 0` and the same at 36 — both words written, with its own descriptor
   numbers, in the order `BitTube::writeToParcel` writes them. The framework still
   reports `status=-9` (`EBADF`) from `DisplayEventDispatcher`, so what is left is
   inside its own `DisplayEventReceiver` initialization: what it does with a
   descriptor it has just been handed. That is a narrow next step, and the two
   ends either side of it are verified.

6. Then the compositing half, which is the rest of the windowing phase
   (ADR-0006, P5 below) and is a subsystem rather than a service:

Three shim bugs were fixed to get here, all recorded in `docs/binder.md`: the
binder object type constants (the old ASCII encoding matched nothing, so no
registration was ever kept), the string16 padding (two bytes for an odd count, not
four), and writing a null strong binder by writing nothing (handing libbinder null
is a SIGSEGV inside the framework's `getService`).

### The two transactions that looked like garbage

For as long as this shim has existed, two streams in every boot were recorded as
undecodable: a 68-byte `BC_TRANSACTION` to handle 0 whose "code" read as
`0x5F504E47` (the ASCII `GNP_`) with no payload, and a call with code
`0x5F4E5446`. They are not garbage and never were:

| code | name | answer |
| --- | --- | --- |
| `0x5f504e47` | `PING_TRANSACTION` (`B_PACK_CHARS('_','P','N','G')`) | an empty reply is yes |
| `0x5f4e5446` | `INTERFACE_TRANSACTION` (`B_PACK_CHARS('_','N','T','F')`) | the interface token |

Neither carries an interface token, which is why the token check rejected them, and
both belong to the protocol rather than to any interface: the first is what
`ProcessState::getStrongProxyForHandle` sends to a handle before it hands out a
proxy for it, the second is what a generated proxy asks before it will use the
object it holds. The shim answers both now, for the service manager and — through
the broker — for the objects the broker hosts. Answering the second with nothing is
what makes a proxy report `UNKNOWN_TRANSACTION` and take the error path generated
for it.

### Another device

`mosaic` itself is the package: build the checkout and `sudo make install`, or build
the PKGBUILD. Nothing per machine is baked in.

The runtime is the part that has to travel, and it now can. Pack it where it exists:

```
tools/bundle/bundle.sh pack ~/.local/share/mosaic/bundle /tmp/packs
  -> runtime-local-x86_64.tar.xz         (309 MB to 126 MB in 5 seconds)
  -> runtime-local-x86_64.tar.xz.sha256
```

Serve those two files from anywhere -- a release, a mirror, a directory on a local
network -- and another machine installs the runtime with

```
MOSAIC_BUNDLE_CHANNEL=https://that.host/somewhere \
MOSAIC_BUNDLE_VERSION=local \
  mosaic runtime fetch
```

The release is the default channel:
`https://github.com/yesheytenzin/mosaic/releases/latest/download`, with version `local`,
so a machine with no configuration at all runs

```
mosaic runtime fetch
```

and gets the runtime. That is the round trip, verified on a fresh work directory: 126 MB
downloaded, sha256 verified, unpacked, and `run.sh` started ART from what arrived.
`mosaic runtime install https://github.com/yesheytenzin/mosaic/releases/download/<tag>/runtime-<v>-<arch>.tar.xz`
is the same artifact pinned to one release.

**If the repository ever goes private**, those download URLs stop working: they are
browser-facing and answer 404 to a private repository with or without a token. Only the
API's asset URLs serve it, so when `MOSAIC_BUNDLE_TOKEN` is set the fetch resolves the
release and both assets through `api.github.com` and downloads them as bytes. Three
things made that path look broken before it worked, and each is where it bit: the API
refuses a request with no `User-Agent`, which reqwest sends none of and curl does; a
release tag is addressed under `/releases/tags/<tag>`, not `/releases/<tag>`, which
reads the tag as an id and answers 404; and the release JSON and an asset's bytes need
different `Accept` headers.

Verified as a round trip: packed the real bundle, served it over HTTP, fetched it into
a work directory that had never seen it, and the status and launch paths found it.
What the fetch does: downloads, verifies the sha256, unpacks to
`<work>/runtime/<version>-<arch>` and records the version. Three things were wrong on
the way and are fixed: `bundle_url` named a `.tar.zst` while the unpacker was xz;
the channel was a constant, so a self-hosted artifact could not be pointed at; and the
version marker was written to the *default* work directory rather than the one given,
so `runtime fetch -w somewhere` put the bundle in one place and the marker in another.

### A user who has no runtime

Three ways, and the first is the one to prefer because it costs nothing per person:

1. **The machine-wide one.** An administrator runs `sudo mosaic runtime install` once
   -- root's work directory is `/var/lib/mosaic` -- and every user's launch finds it.
   Nothing is copied per user.
2. **Their own, built from the machine's image.** `mosaic runtime install` builds into
   `~/.local/share/mosaic/runtime` (309 MB, about 25 seconds). The image at
   `/var/lib/mosaic/images/system.img` is world-readable, and the builder now ships
   with the package at `/usr/lib/mosaic/bundle.sh` -- it used to live only in a
   checkout, which meant a user with an installed Mosaic could not build one at all.
   It needs `debugfs`, `python3`, `clang`, `tar` and `xz`; when one is missing the
   builder says so.
3. **A published bundle, by URL.** `mosaic runtime install <url>` fetches an archive or
   an image, verifies the sha256 published beside it, and either unpacks it or builds
   from it. Nothing local is needed: no checkout, no image, no build tools for an
   archive. The same artifact also works as
   `MOSAIC_BUNDLE_CHANNEL=<where it is served> MOSAIC_BUNDLE_VERSION=<version>
   mosaic runtime fetch`, which is what a published release would use.

   ```
   mosaic runtime install https://example.org/mosaic/runtime-local-x86_64.tar.xz
     -> the archive unpacked, hash verified, and run.sh starts ART from it
   ```

   `bundle.sh pack` produces exactly those two files, so hosting one is a `cp` to
   wherever they are served from.

When none of the three is available the error names all three, rather than leaving
someone to guess.

Two things remain. A bundle carries the image's ART and Bionic, so it is specific to
an architecture: an aarch64 machine needs an aarch64 image and its own bundle. And the
publication step is done and exercised end to end: `tools/publish-runtime.sh` packed,
created the release, uploaded both assets and printed the install lines; the release is
live with the archive's digest matching the local file, the repository is public, and
both `runtime fetch` and `runtime install <url>` were run against it from a fresh work
directory with no token at all.

### More than one user

The package is per machine: one `sudo make install`, and every user gets the command,
the socket unit and the polkit action. What is per user is the work directory --
`~/.local/share/mosaic` with its registry, its apps and its app data -- so each
person installs their own apps with their own `mosaic install`.

The runtime is per machine as well, because it is read-only and identical for
everyone. `sudo mosaic runtime install` builds it once under
`/var/lib/mosaic/runtime` (root's own work directory is the machine's) and every
user's launch finds it: the lookup is this user's runtime first, then the machine's.
Verified by simulating a second user -- a work directory holding only a registry, and
no runtime of its own:

```
runtime status → Runtime bundle shared (x86_64, the machine's) at /var/lib/mosaic/runtime/...
```

`MOSAIC_SHARED_RUNTIME` moves that directory, which is what makes it testable without
a second account.

Two things this does not settle. The UID an app is given comes from its own user's
registry, so two people installing the same APK each reserve one -- they collide, and
the helper sees a system user that already exists. And an app's data directory is per
user while its UID is machine-wide, which is the wrong way round for what Android
does with per-user apps.

### Installing works end to end

`mosaic install <apk>` on the packaged stack now does the whole transaction: the
broker reserves a UID and a data directory, the privileged step allocates the system
user and the directory, and the broker commits. Verified with the real Termux APK:

```
Installed termux.app.v0.119.0.beta.3... (uid 5000, data ~/.local/share/mosaic/apps/...)
mosaic-termux-app-v0-119-0-beta:x:5000:934::/home/mosaic-termux-app-v0-119-0-beta:/usr/bin/bash
drwx------ mosaic-termux-app-v0-119-0-beta 5000 .../apps/termux.app...
```

Two bugs were in the way, both of them this project's own. `default_work` chose
`/var/lib/mosaic` on the strength of `access(W_OK)`, which succeeds inside a
`ProtectSystem=strict` sandbox on a read-only mount, so the broker picked a
directory it could not write and failed with `EROFS` instead of falling back; the
check creates a file now, and the unit names the directory it will actually use. And
the APK path was sent to the broker as given, so a relative path was resolved in the
broker's working directory rather than the user's, which is why
`mosaic install app.apk` in the directory holding it reported "No such file or
directory"; the caller resolves it now.

The runtime bundle has a second way in: `mosaic runtime install <directory>` links a
bundle built by `tools/bundle/bundle.sh build` and records its version, so a bundle
built here does not have to be published anywhere to be used. One trap worth
knowing: the broker runs with `PrivateTmp=yes`, so a bundle left in `/tmp` is
invisible to it while being perfectly visible to whoever built it. Put it under the
work directory (`~/.local/share/mosaic/`), which the unit already allows, and both
processes see the same thing.

What installing does *not* mean: `mosaic launch` is still a stub that says so, and
the package name still comes from the file name rather than the manifest.

## B. `system_server` to completion

9. Every service after `PackageManagerService`: AMS, WMS, ATMS, `StorageManager`,
   `RoleManager`, `PermissionManager`, `ConnectivityService`, `AlarmManager`,
   `JobScheduler` — each announces what it needs, as the fonts did.
10. `PackageManagerService`'s inputs: `/system/etc/permissions`, `/system/etc/sysconfig`,
    `/data/system/packages.xml`, `/data/dalvik-cache`, `privapp-permissions`,
    shared-library lists, and the resource overlays.
11. `installd` ✅ (the boot path: app data directories, sizes, dex and profile
    removal, native library directories, with everything else refused by name) and
    the `vold` equivalent ✗ — storage, mounting and encryption, which nothing on
    the boot path calls yet.
12. `/data` laid out as Android expects, with the right ownership.
13. A property store that persists (`persist.*`).
14. A boot image, or accept the slower imageless start.
15. What `init` does that nothing else does: device permissions, `/dev/ashmem`
    if apps use it.

## C. Running an app process

16. The launcher becomes the app entry point: per-app classpath, `ActivityThread`,
    JNI libraries, and the `Application`/`Activity` lifecycle.
17. APK installation: binary-XML manifest, split APKs, signatures,
    `resources.arsc`, ABI selection, `uses-library`.
18. Per-app UID and data directory through the privileged helper (ADR-0007/0008).
19. DEX optimisation on install, then reuse.
20. The broker starts app processes on demand, tracks and reaps them.
21. Startup time: a pre-warmed template process, once the basics work.

## D. Visible and usable

22. Windowing (ADR-0006): a Wayland-backed implementation of `Surface`,
    `SurfaceControl`, `BLASTBufferQueue`, `SurfaceTexture`, and dma-buf buffers,
    replacing `libgui` and SurfaceFlinger.
23. EGL and the GPU, Skia through `libhwui` (software first), Vulkan for games.
24. Input into `InputManager`: keyboard, pointer, touch.
25. Audio over PipeWire, replacing `AudioFlinger`.
26. Clipboard, notifications, IME, and `Intent` handling on the host.
27. `SurfaceView`, overlays, and Android's window policy in the broker.

## E. Compatibility breadth

28. ARM translation behind `libnativebridge` — FEX-Emu or box64 (ADR-0003).
29. Media: `MediaCodec` (software codecs), `MediaPlayer`, camera, recording.
30. Sensors, location, Bluetooth, NFC.
31. `WebView` — a Tier-4-scale component, likely a permanent gap.

## F. Product

32. Publish the runtime bundle as a pinned artifact and make `runtime fetch` use it.
33. Packaging for at least one distribution; the systemd units; the polkit action.
34. `install`/`launch`/`uninstall`/`query` end to end, with per-app desktop entries.
35. The security model, stated honestly: UIDs plus Landlock and seccomp, no SELinux.
36. Man pages, install documentation, and a compatibility list that names its gaps.
37. An upgrade path for the bundle and for installed apps.

## G. Cross-cutting

38. Decide the path strategy: keep redirecting, or present the paths at install.
39. `/proc` fidelity for the entries apps read.
40. Logging as a real service rather than a harness stand-in.
41. The ratchet as a CI gate: registrars, boot stages, binder calls, apps passing.
42. A hello-world fixture APK and a scripted compatibility matrix.

## H. Never

Google Play services, Play Integrity and attestation, Widevine L1, anti-cheat, and
anything that requires Android's SELinux policy to be enforced.
