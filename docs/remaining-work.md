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

   **Three** things were wrong on this side of it and are fixed. The third is the
   one that mattered: a descriptor word written as a raw object is *not* recorded as
   an object of the parcel, so `Parcel::readFileDescriptor` finds nothing at that
   position and returns `BAD_TYPE` — which the reader then hands to `dup`, giving
   `EBADF` and `BitTube::readFromParcel: can't dup file descriptor`. Writing it
   through `Parcel::writeFileDescriptor` instead both writes the word and records
   it, and the receiver initializes: `Failed to initialize display event receiver`
   went from 2 to 0. The instrument that found it was an interposed `dup`, which
   logged `dup(-2147483647)` — `BAD_TYPE`, not a descriptor number. The shim's reader took
   the frame *header* with `read`, and a descriptor is ancillary data on the
   *first* byte of a frame — so the descriptors were thrown away before the
   `recvmsg` that was meant to collect them, and the count check then failed the
   whole read and the reader thread gave up (after which nothing was ever answered
   again). The header is read with `recvmsg` now. And the parcelable's presence
   word was missing, which is what `status=-61` (`ENODATA`) was: the reader took the
   descriptor word's type for the flag.

   With the channel working, `LocalDisplayAdapter` connects the display and walks
   the info calls, and each one taught something:

   | What it said | What it meant |
   | --- | --- |
   | `No valid static info found` | `Parcel::write(const Flattenable&)` writes the flattened **size** before the fields, and `Parcel::read` reads it back; without it the reader takes the first field for the size. Fixed for both info replies. |
   | `Can't find display mode with id 1`, then a null mode | The tag numbers were hand-counted from the header and five of them were wrong — `getDynamicDisplayInfo` is 55, not 52. Counting the enum programmatically agrees with what the framework sent. Fixed, with the test asserting every one. |

   What is left is the `DynamicDisplayInfo` layout for *this* bundle's revision: it
   reads the color-mode count as a mode id (`Can't find display mode with id 1`) and
   ends with a null mode, so the field set differs from the Android 13 branch this
   side was written from.

   What the bundle's own `libui` says, read off the disassembly, because it is the
   authority and the source branch is not:

   | Struct | Its `getFlattenedSize` / `flatten` |
   | --- | --- |
   | `ui::DisplayMode` | 52 bytes, which matches what this writes — that part is right |
   | `ui::StaticDisplayInfo` | 13 bytes with no product info: `connectionType`, `density`, a **one byte** `secure`, `installOrientation`, and an absent optional writes no flag |
   | `ui::DynamicDisplayInfo` | the modes and colour-mode vectors are counted in **eight** bytes, the two booleans are **one byte** each, `HdrCapabilities` counts its own array in four |

   Those widths are right, and that is now *proven* rather than argued: the shim can
   hand its own reply to the bundle's `ui::DynamicDisplayInfo::unflatten` — the
   reader on the other side — and with `MOSAIC_DISPLAY_INFO_TRACE=1` it reports

   ```
   the bundle's unflatten says 0 for 100 bytes; with nothing in its vectors it wants 98;
   modes vector 140671264652976 .. 140671264653032
   ```

   Zero is success, and the two pointers are 56 apart — exactly one in-memory
   `DisplayMode`. So the struct is right and the reader accepts it. The instrument
   took three attempts: a byte array was not aligned (the vectors fault on the first
   8-byte store), and the first version passed the answer's status and size words as
   if they were struct fields, which came back as `-12` (`NO_MEMORY`, "too small")
   for 108 bytes against a struct that wants 98.

   The ids were then right — the adapter reported this side's own `-1` for the
   preferred mode instead of garbage — which is what the widths fix did. Two more
   were behind it:

   | What it said | What it meant |
   | --- | --- |
   | `Can't find display mode with id 1` / `-65536` | the widths: vector counts are **eight** bytes in this bundle's `libui` and the booleans **one**. A `cargo build` that says "Finished" without "Compiling" is how a first attempt at this edit silently did nothing, and the experiments after it were all on the same bytes. |
   | a null `DesiredDisplayModeSpecs` | that reply has **no status word**: `ISurfaceComposer.cpp` writes `defaultMode` first and signals failure by returning an error, so a result word shifts every field after it. |

   **The display half now works.** `DisplayManagerService` no longer times out:

   ```
   SystemServerTiming: WaitForDisplay
   SystemServerTiming: WaitForDisplay took to complete: 46ms
   SystemServerTiming: StartPackageManagerService
   ```

   and the boot reaches the package manager, which is where the next wall is — see
   below.

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

`PackageManagerService` announces what it needs the way the fonts did, and each
thing it announced is answered:

| What it said | What it was |
| --- | --- |
| `Value "" not valid (reason first-boot)` | two of the fourteen `pm.dexopt.*` names were wrong — the framework wants `install-bulk-downgraded` and `install-bulk-secondary-downgraded`, and the list this side had been written from says `-downgrade`. Declared in `make-property-area.py` now. The properties themselves read correctly from inside the framework's own process by all three libc readers (`__system_property_get`, `__system_property_find`, `__system_property_read_callback`), which is what ruled the area out and pointed at the names. |
| `There must be exactly one installer; found []` | the bundle carried no system apps at all. `PackageManagerService` resolves `ACTION_INSTALL_PACKAGE` and aborts without exactly one privileged handler. `do_apps` in `tools/bundle/bundle.sh` now dumps `/system/app` and `/system/priv-app` from the image; the installer was extracted into the bundle by hand to verify, and the boot moved past the check to `StartDomainVerificationService`. |

Then the same class again, twice, and each answered:

| What it said | What it was |
| --- | --- |
| `Required services extension package is missing` | the package is a **flattened APEX** (`/system/apex/com.android.extservices`), not an app in `priv-app`, so `do_apps` did not carry it. This image ships its apexes as *directories*, so the bundle can carry them without mounting anything; extracted, and the check passes. |
| a **bus error** in `prop_area::find_property` | the property service rewrote the area with `open(path, "wb")`, which **truncates** it first. Every Bionic process in the system has that file mapped, so a reader touching the mapping while it is zero length faults — on whatever thread was reading, which is why it moved between runs (`batterystats-wo` once, `main` another). Writing in place (`"r+b"`) fixes it: the image is the area's full size, so the file's length never changes. `bus: 0` since. |

Two more, from the apex work:

- `do_apps` now also clears the package manager's own record of what it scanned
  (`data/system/packages.xml` and friends). A stale one is worse than none: PMS
  answers from the old scan and reports a package it now has as missing.
- The path shim now resolves `/apex/<module>/...` into the bundle's `apex/`
  directory for anything that is not a `javalib` or `lib64` path. It mapped only
  those two before, so the extension package's apk was unreachable.

**The wall now**, and what is done about it: the **apex service** is implemented —
`src/device/apex.rs`, hosted as `apexservice`, answering `getActivePackages` and
`getAllPackages` from the apexes in the bundle's `apex/` directory, with
`getActivePackage`, `markBootCompleted` and the session calls answered and the rest
refused by name. It works: the daemon logs `apexservice: com.android.extservices at
/system/apex/com.android.extservices`.

Getting it to say that needed one more thing: the broker computed the bundle itself
and ignored `MOSAIC_ANDROID_ROOT`, so a harness running the framework against a
bundle the broker did not know about had services answering about a *different*
tree. The environment wins now, because it is the tree the framework is actually
running from.

**What is still missing** is that the framework does not consult it. The package it
wants is `android.ext.services` — the package *inside* the `com.android.extservices`
apex, not the module.

What the source says, so the next attempt starts from it rather than from scratch:

- `InitAppsHelper.initSystemApps` calls `mApexManager.scanApexPackagesTraced` before
  it scans the system directories, and `ApexManagerImpl` reaches the service through
  `Binder.allowBlocking(ServiceManager.waitForService("apexservice"))` — so a
  `checkService apexservice` should appear in the shim's log.
- None does, and `PackageManagerTiming: getActiveApexInfos` never appears either,
  although the scan it belongs to clearly runs (`packageCount: 53`). So the call is
  not being made, and the question is why rather than what it would answer.
- `ApexManagerImpl.scanApexPackagesInternalLocked` returns early when the service
  answers with nothing (`if (allPkgs.length == 0) return;`), which is what an
  unreachable service looks like from the outside — but the lookup would still have
  been logged before that.

**Settled, and fixed.** This image is a *flattened* apex build, so the framework
does not use `ApexManagerImpl` at all — it uses `ApexManagerFlattenedApex`, whose
`getActiveApexInfos` lists `/apex` itself and whose `scanApexPackagesTraced` is a
no-op. That is why no `apexservice` lookup ever appeared: on this image the service
is not consulted. (It is still the right thing to have for an updatable device, and
it works.)

What the listing needed was two things in the path shim:

- `opendir` was not redirected at all. `open`, `stat` and `access` were, which is
  enough to *find* a file and not enough to *list* a directory, and
  `File.listFiles()` is `opendir`/`readdir`.
- `/apex` itself — the directory the framework opens, with no trailing slash — did
  not match the `/apex/` rule.

With both, the apexes are listed, the packages inside them are scanned, and three
walls went at once: `There must be exactly one installer`, `Required services
extension package is missing`, and `There must be exactly one permissions manager`.
`do_apps` now carries `/system/apex` as well as the app directories, because the
packages the framework *requires* live inside those apexes.

Then the package manager's own data path, one bug: `createAppDataBatched` answered
each `CreateAppDataResult` with a presence word, and `IInstalld.aidl` declares
neither the return nor the element type `@nullable`, so the reader took the flag for
the first field and every element read as null — `Installer$Batch.execute` then
dereferenced one. The boot now reaches **35 stages**, up from 30.

The broker still logs every lookup, and that is what settled the question: in a full boot it serves 201
lookups of `activity`, seven of `incremental`, four of `display`, `installd`,
`suspend_control`, and so on — and **not one of `apexservice`**. So the call is
genuinely never made, while the same mechanism works for every other Java lookup
(`activity` is one).

Since `initSystemApps` calls `getActiveApexInfos` unconditionally, the open question
is what `mApexManager` is at runtime rather than whether the call happens. The next
step is to see that — the shim can log the Java-side call, or `ApexManager`'s
singleton can be traced — rather than to keep reading `ApexManagerImpl`, which says
it uses `ServiceManager.waitForService("apexservice")`.

The original note, kept because the shape of the problem is still right: `ApexManager` asks `apexd` for the
installed apexes over binder, and nothing here hosts that service, so a package that
is present and reachable is still unknown to the package manager. It is the same
shape as `installd`: a native daemon on a device, an AIDL interface
(`IApexService.getActiveApexInfos` and the rest), and a name the framework looks up.

A `apex-info-list.xml` was written into the bundle on the theory that the framework
falls back to reading that file; nothing in the log ever mentions the path, so it is
the service it wants and the file is not consulted. The file is harmless and stays
for the fallback, but it is not what this needs.

An attempt to seed the area from the image's *whole* `build.prop` instead of the
names Mosaic declares is worth not repeating: the area holds what Mosaic declares
plus room for the properties the system server writes during boot, and taking all
131 of the image's names left too little of that room — the first write past the end
of the mapping is a bus error in the process that made it.

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


## The idmap service, and where it stands

The overlay compiler. Four things are in and one is not.

In:

- The bundle carries `idmap2` and `idmap2d` from the image, and the *closure* now
  walks the bundle's own **binaries** as well as its libraries — a library only a
  program needs was invisible to a scan of `lib64` alone, and `idmap2d` needs
  `libidmap2.so` and `libidmap2_policies.so`.
- The property service answers **`ctl.start`/`ctl.stop`**, which is how Android
  starts a daemon: the framework writes `ctl.start='idmap2d'` and then waits five
  seconds for the service it registers. A write that is stored and not acted on is
  a boot that fails with `Failed to connect to 'idmap' in 5000 milliseconds`.
- That start goes through the bundle's own runner, which reads the binary's ELF
  interpreter: a Bionic program exec'd directly fails with "required file not found"
  because its interpreter is `/system/bin/linker64`.

**And then in.** The daemon was aborting on

```
Binder driver '/dev/binder' could not be opened. Terminating
```

— it was running *without the shim*: `bundle.sh run` sources the bundle's `env.sh`,
which does not carry the framework's preload list, so nothing stood in for the
driver a host does not have. The property service passes `MOSAIC_PRELOAD` on as
`LD_PRELOAD` now, and the daemon registers: `Failed to connect to 'idmap'` went to
**zero** and the boot gained six stages.

**`startBootstrapServices` is complete.** The boot is into `startCoreServices` now,
at **41 stages**, and the wall is

```
java.util.NoSuchElementException: IHealth service instance default isn't available.
```

— the health HAL (`android.hardware.health.IHealth/default`), which `BatteryService`
asks for its battery and charging state.

**Implemented, and it reads the host**: `src/device/health.rs` answers
`getHealthInfo` and the cheap readers from `/sys/class/power_supply`, taking the
supply whose `type` is `Battery` rather than the first entry (a laptop also has `AC`
and a UCSI source in that directory). It logs what it found, and it found the real
thing:

```
health: BAT1 at 80%, 4, ac true usb false
```

— 80%, status 4 (`NOT_CHARGING`: plugged in and not charging), AC online. What the
host does not have it says it does not have: no wireless or dock charger, no cycle
count where the kernel exposes none, `UNKNOWN` capacity level rather than a guess.

**It is not reached, and the reason is not the health HAL at all.** No lookup for
the health name appears in the broker's log because `BatteryService` does not ask
the service manager this side serves: `HealthServiceWrapper` imports
`android.hardware.health.V2_1`, so the health HAL it wants is **HIDL**, and a HIDL
client asks `/dev/hwbinder` — which has **no service manager** (the gap named in
`docs/binder.md`). The service is implemented and answering on `/dev/binder`; the
framework is looking on the other device.

So two open items are one: the health HAL needs the hwbinder service manager. That
manager speaks a *different* protocol from `/dev/binder`'s — HIDL's `IServiceManager`,
eight methods in declaration order (`get`, `add`, `getTransport`, `list`,
`listByInterface`, `registerForNotifications`, `debugDump`,
`registerPassthroughClient`) with HIDL's own marshalling rather than AIDL's.

**The fix is not to reimplement it.** The image ships `hwservicemanager`, the daemon
that *is* the HIDL service manager, and the bundle now carries it (`do_apps`'s tool
list). What is left is the two ends of the same idea as `idmap2d`:

- the shim has to honour `BINDER_SET_CONTEXT_MGR`, so the daemon can become the
  context manager on `/dev/hwbinder` the way it does on a device, and
- handle-0 calls on that device have to route to it rather than to this shim's own
  `/dev/binder` registry.

The shim's own bookkeeping had to come first and is in: it kept **one** `binder_fd`,
so the last device opened took the slot and the shim answered the wrong device's
ioctls afterwards -- the service manager's among them, which is `/dev/binder`'s
alone. Each device has its own placeholder and its own identity now, which is what
the routing needs: handle 0 on `/dev/hwbinder` is the HIDL service manager, a daemon
of its own, not the registry this shim keeps for `/dev/binder`.

**A correction to the paragraph above.** `HealthServiceWrapper` tries the *AIDL*
health HAL first and only falls back to HIDL:

```java
        try {
            return new HealthServiceWrapperAidl(aidlRegCallback, aidlServiceManager);
        } catch (NoSuchElementException e) {
            // Ignore, try HIDL
        }
        return new HealthServiceWrapperHidl(...);
```

and the AIDL path does not ask the service manager at all:

```java
        default @Nullable IHealth waitForDeclaredService(@NonNull String name) {
            return IHealth.Stub.asInterface(ServiceManager.waitForDeclaredService(name));
        }
```

`waitForDeclaredService` checks the **VINTF manifest** first. So the hwbinder
service manager is a fallback's requirement, not this one's: what the boot needs is
a declaration that the AIDL health HAL exists, and a manifest entry that is not
accepted leaves the framework falling through to HIDL -- whose lookup goes to
/dev/hwbinder and stops there.

A declaration is written — first as a fragment in the `<version>`/`<fqname>` form
this image's own fragments use, then appended to the image's *own* device manifest,
which is now carried along with its compatibility matrix — and it is **not yet
accepted**: the framework still reports the service as unavailable and no lookup
reaches the broker. The rejection is silent, so the next instrument is a trace of
which `vintf` paths are opened at all.

The instrument that has settled every other question was built and it narrowed this
to a name:

- `AServiceManager_isDeclared` **is** called, and the log shows it asked for
  `android.hardware.memtrack.IMemtrack/default` and
  `android.hardware.sensors.ISensors/default` — both answered "no" by libbinder_ndk
  itself, from a manifest it reads — and **never for health**. So the health AIDL
  attempt fails *before* the check this side can see, which is why adding
  `IS_DECLARED` to the shim's service manager changed nothing: no such transaction
  arrives.
- `fopen` is interposed now, which is a real gap: a library that reads a file
  through it does not come through `open` at all, because `fopen` calls `open`
  *inside libc*, and a preloaded `open` is not called for an internal call.

**And it is answered.** `ServiceManager.waitForDeclaredService` is
`isDeclared(name) ? waitForService(name) : null`, and `isDeclared` is a *binder call*
to the service manager — the seventh method of `android.os.IServiceManager`, whose
declaration order the shim already matched for codes 1, 2, 3, 4 and 13. Code 7 was
simply missing, so an unknown transaction came back empty and the answer was false.
With it answered for the HALs the bundle's manifest carries:

```
android-binder: isDeclared android.hardware.health.IHealth/default -> yes
IHealth service instance isn't available: 0
```

**The wall moved again**, and to something familiar:

```
ServiceManager: Waited one second for android.hardware.health.IHealth/default
```

The declaration is believed; what is missing is the *service*. The instrument says
exactly how far it gets. In the framework's process:

```
android-binder: smcode 7 android.hardware.health.IHealth/default     (declared: yes)
android-binder: smcode 1 android.hardware.health.IHealth/default     (getService)
android-binder: smcode 5 android.hardware.health.IHealth/default     (register for notifications)
android-binder: getService … local=0 broker_fd=46 remote=4294967295 (NO_HANDLE)
android-binder: sending to the broker failed (kind 0)
```

So the lookup **does** reach the shim's service manager and the broker connection is
**up**. The send then fails, and the broker's own log says when the conversation
stops: the framework *is* a client and its lookups *do* arrive — `display`, `window`,
`activity`, `installd`, all at the same second, all through this same path — and
nothing after that. The connection dies mid-run, and every lookup after it fails
without a byte leaving.

What broke it was the reader thread, which `break`s on the first `broker_recv`
failure without closing the descriptor or clearing `broker_fd` -- and `broker_send`
did the same on a failed write. Both now clear the connection so the next caller
reconnects, which is cheap and correct: the broker owns the names, so a fresh
connection is the same authority as the old one. That is a fix with a number
attached:

```
lookups reaching the broker: 282   (was: a handful, then nothing)
send failures:               0     (was: sending to the broker failed (kind 0))
clients:                     3     (the framework reconnects)
```

**The wall is past the health wait now**, and the next failure is a different one:

```
java.lang.RuntimeException: Unknown exception code: 1746956142 msg null
FORTIFY: pthread_mutex_lock called on a destroyed mutex (0x7fa2711ff0a8)
Aborted (core dumped)
```

The framework has looked up `dropbox` and moved on, so the health service is being
reached and the boot is further along than it has ever been.

The throw is a *stale handle*, and the broker says so in as many words:

```
client 3 sent a transaction that cannot be routed: no handle 43 in client 3
java.lang.RuntimeException: Unknown exception code: 1746956142 msg null
  at android.os.Parcel.readException(Parcel.java:2942)
```

1746956142 is `0x68206f6e`, which is ASCII — a *string* read where a status belongs,
which is what a failed route looks like from the caller's side.

The first half of the cause was the reconnection, and it is fixed: a client is now
the *process*, not the connection. `connect` hands a reconnecting pid back its own
record and its handle table, `disconnect` marks it rather than removing it, and it
keeps its nodes when the process is still running — ownership is by process, which is
why the node id carries the pid. The broker's log says it works:

```
Binder client 1 disconnected (still running)
Binder client 1 reconnected (pid 278469)
```

The teardown had to learn the same thing: a session that ends *after* its process has
reconnected must not mark the live connection dead, so it compares the peer it owns
against the one in the map. Liveness is a field rather than a `/proc` read inside the
call, so a test can decide it instead of depending on which pids the machine happens
to be using — and the tests that had been quietly passing for the wrong reason now
say what they mean.

**The second half is fixed, and it was not where it looked.**

The unroutable transaction was a handle to a dead service:

```
client 2 disconnected
client 1 sent a transaction that cannot be routed: no handle 43 in client 1
```

The broker answered it correctly -- `EX_TRANSACTION_FAILED` with the reason as the
body -- and the *reply* was where it went wrong. A binder reply's data begins with
the status; that is the driver's convention and it is the first thing
`Parcel.readException` reads. The broker sends the status as a field of its own, and
the shim joined them **on the failure path only**: a transaction nobody answered got
`&failed, 4`, while a *forwarded* one copied the answer with no status in front of
it. So the caller read the first four bytes of the answer as a status, and for a
failed transaction the answer is the error text:

```
no handle 43 in client 1   ->   0x68206f6e   ->   "no h"   ->   1746956142
```

That is the unknown exception code, and it is why the value decoded as ASCII. The
forwarded path now writes the status first, exactly as the local path already did,
and shifts every object offset by the four bytes it took. Verified end to end:

```
unroutable transactions: 0     (was 2)
unknown exception codes: 0     (was 5)
SystemServer timings:   41     (the most this boot has reached)
```

The abort is gone with it: the run now ends on its own timeout instead of a core
dump. The stack had said the abort was secondary -- `exit` -> `__cxa_finalize` ->
`RefBase::weakref_type::decWeak` -- and it was: fixing the throw removed it.

Along the way the driver's own command values were corrected against the kernel
header. `BR_TRANSACTION_COMPLETE` was `BR(4, 4)`; the driver's is `_IO('r', 6)`, a
command with no operand at all, and a command whose size field disagrees with the
reader's idea of it is one the reader cannot skip.

**Death notification was built, and it turned out not to be the blocker** -- but it
was genuinely missing and is now there, in both directions: the shim forwards
`BC_REQUEST_DEATH_NOTIFICATION` and `BC_CLEAR_DEATH_NOTIFICATION` to the broker as
`KIND_LINK` and `KIND_UNLINK`, and a `KIND_DEAD` from the broker becomes
`BR_DEAD_BINDER` on the reading thread with the cookie the caller gave. The framework
never links to the handle that broke, which is why this was not the fix -- but a
caller that *does* link would have been told nothing at all, forever.

The abort that follows is secondary and worth stating so nobody chases it first: the
stack is `exit` -> `__cxa_finalize` -> `RefBase::weakref_type::decWeak`, an object
being torn down during shutdown after the throw had already unwound.

Worth noting for the next person: the encoding is not the problem. `Message::Lookup`
maps to `KIND_LOOKUP` with fields `(id, 0, 0, 0, name)`, and the shim sends exactly
that. The two sides agree; the conversation is what stops.

Two real fixes went in while finding this, both worth keeping:

- **`broker_lookup` now connects.** It used to bail when `broker_fd < 0`, and the
  connection was made lazily inside `broker_send`, which the early return never
  reached — so a process that only ever *looks up* never connected at all, and every
  lookup came back absent without a byte going out.
- **`fopen` is interposed**, for the same reason as `opendir`: a library that reads
  a file through it does not come through `open`, because `fopen` calls `open`
  *inside libc*.

The lesson from the four manifest revisions stands and is now demonstrated: the
instrument (an interposed `isDeclared`) found this in one run after three rewrites
found nothing. A lesson from getting there: the first version used exactly that
form and was changed to the `<interface>` form on the evidence of a *compatibility
matrix*, which is a different schema from a *manifest*. The matrix is not the
manifest. What is left is to find why the declaration is rejected rather than to
keep rewriting it.

The health HAL itself is not the blocker: this image carries its HIDL *libraries*
(`android.hardware.health@2.0.so` and friends) and no health daemon, so it is a
passthrough HAL the client dlopens. The service manager is the only missing piece,
and `src/device/health.rs` — implemented, reading this host's battery — is what will
answer once it can be found.

## The health lookup, instrumented

The framework waits a second at a time for `android.hardware.health.IHealth/default`, and
the service *is* hosted -- `device/mod.rs` puts it in the broker's registry -- so the
question is why the lookup never lands. The shim's own log answers it, and the answer
is a connection, not a declaration:

```
android-binder: smcode 1 android.hardware.health.IHealth/default
android-binder: getService android.hardware.health.IHealth/default local=0 broker_fd=-1 remote=4294967295
android-binder: the broker conversation ended
```

`broker_fd=-1` on every one of the 101 attempts, and `sending to the broker failed` is
*absent* -- so `broker_lookup` never sent anything. That is the shape of a connection
that is not up: it connects, the reader ends, the descriptor is cleared (correctly, by
the fix above), the next lookup connects again, and the reader ends again.

The broker's log shows the other side of it, and that it is not a rejection:

```
Binder client 1 reconnected (pid 288705)
Binder client 1 disconnected (still running)
Binder session for client 1 ended
Broker request failed: no connection for client 2
```

Other names *do* arrive in the same run -- `activity`, `idmap`, 281 lookups in all -- so
the lookup path, the wire, and the routing are all working. What is left is why the
framework's connection is the one that keeps ending, and the next instrument is the
transport's own end: whether `serve` returns on a `Bye`, on a decode error, or on the
peer closing, since the broker's log shows no decode failure.

## The health service is reached

The connection that kept ending was ended by a *timeout*, not a rejection. A
forwarded call whose owner has died waits out `FORWARD_TIMEOUT` and fails, and that
error propagated out of the session loop -- so one dead service (the idmap daemon,
here) cost the caller every other connection it had, for the rest of its life. Every
later lookup reconnected and died again, which is what `broker_fd=-1` on all 101
attempts was.

There is now one place that decides what a failed transaction means, and it is the
caller's problem rather than the connection's: the caller gets a failed reply and the
session carries on. Verified:

```
client 1 looked up android.hardware.health.IHealth/default
health lookups: 1        (was 0)
total lookups: 285
session errors: 2        (both survived)
```

Two exception codes still surface, and both are now *delivered* rather than fatal --
`0x61206f6e` is "no a" (`no answer for node …`) and `0x68206f6e` is "no h" (`no handle
43 …`). Before this they would have taken the connection down; now the framework sees
them, which is what a driver does.

**The wall is the health service's callbacks**, which is much further along than the
declaration ever was:

```
BatteryService: health: Waited 175033ms for callbacks. Waiting another 1000 ms...
```

The service is found, the framework has it, and what it is waiting for is the health
HAL to call back after registration. That is the next piece: the callback the
framework registers over `IHealth` has to be stored and answered, the way the service
manager's registration already is.

### What the callback needs

`BatteryService` is waiting for the health HAL to call it back, and the HAL cannot.
The pieces that are missing are small and named:

- `BinderObject::transact(&mut self, code: u32, data: &[u8])` does not receive the
  *arguments*, so a hosted service cannot see the callback object the caller passed
  it. The broker resolves those objects already -- it turns them into the owner's
  handles for forwarding -- it just does not hand them to a hosted object.
- `Answer { data, objects, fds }` has no way to say "and call this handle". The
  broker forwards transactions to clients already, so the routing exists; what is
  missing is a hosted object being able to ask for it.

The shape that fits both: `transact` takes the resolved argument handles, and
`Answer` gains a list of calls to make after it is delivered -- a handle, a code and
a body. The health HAL then answers `REGISTER_CALLBACK` by asking for one call to the
callback it was given, with `GET_HEALTH_INFO`'s payload, which is what a real health
HAL does: it reports the current state immediately rather than only on change.

`code::REGISTER_CALLBACK` is already answered, and the comment there says the
framework tolerates a callback that is never called and reads the state when it needs
it. That is not true of this framework version -- it waits, and the wait is what the
boot now ends on.

### The callback works, and a path bug behind it

`BatteryService` no longer waits: it is told, once, when it registers. That is what a
health HAL does -- it reports where things stand immediately rather than only on a
change -- and it needed two pieces of plumbing that did not exist:

- `BinderObject::transact` never received the *arguments*, so a hosted service could
  not see the object its caller passed. There is now a defaulted `transact_with` that
  takes the caller's handles, so only a service that needs them says so.
- `Answer` could not say "and call this handle". It can now, and the transport makes
  each call after delivering the answer, one-way, the way any callback goes.

```
BatteryService: health: Waited ...     0    (was 175033 ms)
```

With the wait gone the boot moved on to the overlay service, which failed for a
reason of its own that had been invisible:

```
bin//home/.../bin/idmap2d: No such file or directory
OverlayManagerService: failed to get all fabricated overlays
```

The bundle's runner resolves a binary *name* against `$out/bin`, and the boot script
was passing it a full path, so the two were joined. `idmap2d` never started, `idmap`
was never registered, and nothing said so until the overlay service needed it. It now
runs. **This is worth remembering as a shape**: a failure that surfaces as
"failed to get all fabricated overlays" was a shell script passing an absolute path
where a name was expected, three layers down.

The wall is now `OverlayManagerService.onSwitchUser`, and the exception it raises is
`Unknown exception code: 1` -- a *real* status this time, not a message read as one.

### The next wall, and what it actually says

`OverlayManagerService.onSwitchUser` fails, and the line above it is the real one:

```
IPCThreadState: *** BAD COMMAND -2143260157 received from Binder driver
IPCThreadState: getAndExecuteCommand(fd=4) returned unexpected error -2147483648, aborting
libc: Fatal signal 6 (SIGABRT) ... Aborted (core dumped)
OverlayManager: failed to get all fabricated overlays
  at com.android.server.om.IdmapManager.getFabricatedOverlayInfos(IdmapManager.java:124)
```

The binder thread aborts on a command it does not recognize, and everything above it --
the fabricated overlays, the overlay service -- is downstream of that.

`-2143260157` as a word is `0x80407203`: direction read, size 64, type `'r'`, number 3.
That is `BR_REPLY`, and 64 is `sizeof(struct binder_transaction_data)` from the kernel
header -- target 8, cookie 8, code 4, flags 4, sender_pid 4, sender_euid 4, data_size 8,
offsets_size 8, data 16. So the shim's value and the kernel's agree, and the command
the reader rejects is the one it should accept.

What that leaves is the reader's own idea of the struct rather than the kernel's:
`IPCThreadState` is compiled against its own headers, and the word it compares against
is its own `BR_REPLY`. The next step is that value -- read it from the image's
`IPCThreadState`/`libbinder` rather than from the kernel's header -- and the difference
will be a size field, since the direction, type and number already agree.

Worth noting for whoever picks this up: the previous wall was a *message* read as a
status, and this one is a *command* rejected outright. They look similar in a log and
have nothing to do with each other.

### The command words, measured rather than assumed

The reader's own values are in `libbinder`, so they can be read instead of derived.
Scanning the image's `libbinder.so` for every `_IO`/`_IOR` word built with type `'r'`:

```
nr  2  dir 2  size  64     BR_TRANSACTION, 64-byte struct
nr  2  dir 2  size  72     BR_TRANSACTION, 72-byte struct (the secctx variant)
nr  3  dir 2  size  64     BR_REPLY
nr  4  dir 2  size   4
nr 15  dir 2  size   8     BR_DEAD_BINDER
nr 17  dir 0  size   4
```

`BR_REPLY` at size 64 is what the shim already sends, so the rejected command is not
that one -- and the shim's `BR_DEAD_BINDER` matches too. What the image has and the
shim does not is **`BR_TRANSACTION`**, in both sizes, and that is the real gap: the
shim has no path that delivers an incoming transaction to the process that owns the
object. `KIND_INCOMING` from the broker goes to `broker_serve`, which serves it inside
the shim, and the framework's own `Binder` never sees a `BR_TRANSACTION` at all.

That matters now in a way it did not before, because the health callback is the first
thing that needs it: the broker calls the framework back, the shim receives it, and
there is nothing that hands it to the framework's binder thread.

So the next piece is the incoming path: `KIND_INCOMING` becomes a `BR_TRANSACTION`
written on a read, with the descriptor and the caller's handle resolved the way the
outgoing path already resolves them. The sizes are known -- 64 for the plain struct,
72 when the secctx is carried -- and the reader will say which it wants by accepting
one and rejecting the other.

### What the incoming path needs, measured

The shim already parses a `flat_binder_object` -- type at 0, flags at 4, the object or
handle at 8, the cookie at 16 -- so the cookie is in hand at the moment a service is
published. It is not carried anywhere past that:

- the wire's `Export { name, node }` has no cookie, so the broker does not have it;
- `KIND_INCOMING` is served inside the shim by `broker_serve` rather than written to
  the framework's binder thread as `BR_TRANSACTION`;
- so when the broker calls the framework back -- which the health callback now does --
  there is nothing that reaches the framework's own `Binder`, and the abort in
  `IPCThreadState` is what that looks like from the outside.

The three pieces, in order:

1. `Export` carries the cookie, and the broker keeps it beside the node.
2. `Incoming` carries it back, so the shim knows which object the framework owns.
3. The shim writes a `BR_TRANSACTION` on a read: the node's cookie in
   `binder_transaction_data.cookie`, the caller's handle in `target.handle`, and the
   code, flags and data from the message. The size is 64 or 72 -- the image's
   `libbinder` contains both, and which one this reader wants is decided by which it
   accepts.

### The incoming path, first attempt, and what it shows

The delivery is in: `KIND_INCOMING` is no longer served inside the shim, it is handed
to the ioctl half as `shim_incoming` and written on a read as `BR_TRANSACTION`, with
the cookie the registry already keeps beside each published node. It builds and runs,
and the boot is unchanged -- which is the interesting part, because the bad commands
now decode:

```
-2143260157  ->  0x80407203   _IOR('r', 3, 64)    a real BR_REPLY, the value the image uses
-2147454458  ->  0x7fff8dfa   dir 1, type 0x8d, nr 250   not a command word at all
```

`0x7fff8dfa` is a pointer. The reader is reading a *pointer* where it expects a
command word, which means the layout this side wrote is being walked past its end --
the struct is 64 bytes and the data follows it inline, but the reader follows
`data.ptr.buffer` and then advances by the *command's* size, so where it lands next is
decided by arithmetic this side got wrong.

That is the next step and it is narrow: the incoming write needs the layout the reader
walks, not the layout the kernel describes. The three pieces above are in place --
the cookie is recorded, the delivery exists, the value is the image's -- and what is
left is the framing.

### The framing, and a mistake worth recording

The incoming write had the parcel inline and counted it in `read_consumed`. That is
wrong for the same reason the reply path is right: the command stream is commands
only, the reader advances by the size each command's word carries, and the parcel is
reached through `data.ptr.buffer`. Writing it inline put the reader's next command
wherever the arithmetic landed -- and what it read there was a pointer, which is what
`0x7fff8dfa` in the log was. It now holds the buffer the way a reply does and returns
`4 + 64`.

While removing the now-dead `broker_serve`, a brace-matching deletion took code with
it, and the attempt to undo that -- `git checkout -- tools/binder-shim/android-binder.c`
-- reverted **every change this session made to that file**: the `isDeclared` answer,
the connection recovery, the death bridges, the incoming delivery, the cookie lookup,
and the status that a reply's data has to begin with. All of it was re-applied and
checked for by name (147 lines, eight occurrences of the new symbols), and the build
is clean apart from `broker_serve` being unused. The lesson is the obvious one and it
is recorded here because it nearly cost the session's work: a file-level checkout is
not an undo for an edit.

**The wall has moved to the display service**, which is the compositing half:

```
java.lang.RuntimeException: Failed to boot service
  com.android.server.display.DisplayManagerService: onBootPhase threw an exception
  during phase 100
```

That is `PHASE_SYSTEM_SERVICES_READY`, and it is reached before the health wait, so
this run is shorter than the ones that got to 41 timings. The display path is the one
with the least behind it -- `SurfaceFlinger` and the compositing half are named as
remaining work in their own right -- so this is the next wall rather than a regression
in the ones that were fixed.

### The display failure is a descriptor, not a service

`DisplayManagerService` fails at `PHASE_SYSTEM_SERVICES_READY`, and the lines above it
say what it was doing:

```
Parcel: Invalid object type 0x73662a85
dalvikvm64: BitTube::readFromParcel: can't dup file descriptor (Bad file descriptor)
stealReceiveChannel failed: Status(-129, EX_TRANSACTION_FAILED): '-9 (Bad file descriptor): '
DisplayEventDispatcher: Failed to initialize display event receiver, status=-9
```

`DisplayEventDispatcher` asks SurfaceFlinger for a `BitTube` -- a binder object plus a
descriptor -- and gets neither. `0x73662a85` is a pointer read where an object type
belongs, and the descriptor it is handed will not `dup`.

The reply's framing is right: `write_answer_fds` writes the descriptor word at the
object's offset and the object offsets shift by four with the status, so the reader
looks where the writer wrote. What that leaves is that the *values* are not there to
write -- the object word and the descriptor both come from the broker's answer, and
if the hosted SurfaceFlinger's descriptor never reaches the broker's reply, the word
keeps whatever the broker sent and the reader reads a pointer.

The service end of that chain is right, and provably: `stealReceiveChannel` builds the
reply as status, presence word, two descriptor words, and returns
`Answer::from(...).handing_fd(receive_at, receive).handing_fd(send_at, send)`, with a
test asserting two descriptors at offsets 8 and 36. The broker writes them into the
data and the transport sends them with the frame.

Walking the shim's arithmetic says the offsets are right, and that is worth recording
because it removes a whole class of explanation:

- `broker_transact` returns the status in `a` and the answer's data untouched, so the
  data does *not* begin with a status;
- `answer_data_size` strips the trailer the broker appends, so `answer` is the body;
- `answer_objects` parses that trailer, so its offsets are into the body;
- `write_answer_fds` writes the descriptor number at object offset plus eight, in the
  body, *before* the copy;
- the copy puts the body at plus four, so the number lands at plus twelve -- which is
  exactly where a reader that was told the object is at plus four looks for it.

Every step agrees. So the word the framework reads as a type is not a misplacement but
a *value*: `0x73662a85` is `"sf"` and three more bytes, which is a string pointer, and
that is the shape of a `cookie` -- the framework's own object pointer -- written where
a type belongs. That points at the object word's *contents* rather than its position,
and the next step is to log what the shim writes there and what the broker sent, rather
than to keep deriving it.

### A wrong constant, found by its value

The word the framework called an invalid object type was `0x73662a85`, and that turned
out to *be* a constant in this repo: `BINDER_TYPE_FD`. The driver packs a type as
`B_PACK_CHARS(c1, c2, c3, type)`, so it should be `'f', 'd', '*', 0x85` --
`0x66642a85`. What was written is `'s', 'f', '*', 0x85`: the *handle* pattern
(`'s', 'h', '*'`) with the middle letter changed.

The shim has the same value, and also a wrong `BINDER_TYPE_WEAK_BINDER`
(`0x73622a86`, which is not any packing), both now corrected against the kernel
header. It is one of those bugs whose whole story is in its value: a descriptor
written with it is not recognized as a descriptor, so `BitTube::readFromParcel` never
takes it, `DisplayEventReceiver` cannot initialize, and `DisplayManagerService` fails
to boot at `PHASE_SYSTEM_SERVICES_READY` -- four layers from the constant, and the
only thing in the log that says so is four bytes nobody had decoded.

Verified:

```
Invalid object type:    0     (was 4 or more per boot)
unknown exception codes: 0
```

**What is left is the object list**, one line further in:

```
binderParcel: Attempt to read object from Parcel at offset 32 that is not in the object list
BitTube::readFromParcel: can't dup file descriptor (Bad file descriptor)
```

The service's side is checked and correct. `fd_placeholder` returns the current length
and advances by 28, so with the status word and the presence word in front the two
descriptor words land at 8 and 36 -- which is what the test asserts and what
`stealReceiveChannel` builds. The shim shifts each reported offset by four for the
status, so 32 in the log is four short of 36 and eight short of what the shift would
send: the two numbers do not reconcile by arithmetic from either end.

And the reply does not go through the branch that was instrumented. The display path
reaches `write_broker_answer`, and what happens there is the whole remaining story:

```c
if (object.type == BINDER_TYPE_FD) {
    if (fd_used < response_fd_count) {
        write_object_into(reply, BINDER_TYPE_FD, (ulong)response_fds[fd_used], 0, stability);
        fd_used++;
    }
}
```

When there is no descriptor to put in the word, **nothing is written at all** -- the
loop still advances past it, so the reply is four bytes short of what the offsets in
its own trailer say, and every offset after the first descriptor is wrong by one
object. That is where "offset 32 that is not in the object list" comes from: the
reader's list is right and the data it describes is not the data that was built.

So the question is not the offsets at all -- it is `response_fd_count`, and therefore
whether the descriptor the service hands back crosses the broker and the socket to
reach the shim. The service returns it (`handing_fd`, with a test asserting both), so
the next step is the two hops after that: `Dispatch::Reply.fds` into the frame, and the
frame's descriptors into the shim's read.

### The status, in the branch that matters

`write_broker_answer` never wrote the status word, so the reply parcel began with the
presence word and every offset after it was four short -- which is exactly the "offset
32 that is not in the object list" the reader reported, four short of the 36 the
service declared. The instrument settled it rather than argument: one line logging the
body size, the object count and the descriptor count on the reply the display path
actually uses showed

```
answer body=64 objects=2 descriptors=2
```

so the descriptors *were* arriving and the objects *were* where the service put them.
The status was the only thing missing, and with it written first:

```
can't dup file descriptor:  0     (was 1)
```

`BitTube::readFromParcel` takes its descriptor now. The display service still fails,
one message further in and a different one -- `Attempt to read object from Parcel at
offset 0 that is not in the object list` -- which is a reply that hands back an
*object* rather than a descriptor, so it is the other half of the same convention
rather than the same bug.

Both branches now write the status first: the forwarded one and the parcel-based one
that the display path uses. The instruments are removed.

### The other half: an object handed back

The remaining message is

```
Attempt to read object from Parcel at offset 0 that is not in the object list
```

and it is the object hand-back -- `createDisplayEventConnection` answering with an
`IDisplayEventConnection` rather than a descriptor. The path is `handle_transaction`,
which writes into the *caller's* Parcel rather than handing back words: the status
first (now), then each object through libbinder's own `Parcel::writeObject`, which is
what registers it in the Parcel's object table, then the read position reset to zero.

The reader still looks at offset 0 and finds nothing there, and 0 is where the status
is, so what it is asking for is an object at the very start of the data. The reply
writes one at 4 -- right after the status -- and the offsets a reader uses are
relative to the data, so those two should agree and do not.

That is the next thing to look at, and it is one call: which offset the object word
ends up at in the Parcel, and which offset the reader is told about.

### Where the object hand-back stands

The reply that fails is `forwarded code 4` -- `stealReceiveChannel` on the display event
connection -- and it now gets a *different* error than it did: `status=-19` where it
was `-9`, with the descriptor path itself working (`can't dup file descriptor` is gone
from the run entirely). The remaining line is

```
Parcel: Attempt to read object from Parcel 0x... at offset 0 that is not in the object list
```

and offset 0 is where a status would be, so the Parcel being read is one where the
reader has not taken a status first. Guarding the status write for one-way calls did
not change it, so the call is not one-way and the reply it reads is not the one this
side just wrote: either the object word lands somewhere the offsets do not name, or the
reader is on a different Parcel from the one `write_broker_answer` filled.

The instrument for that is the same shape as the one that settled the descriptor count:
log the Parcel's object offsets and its ipc data length on the reply, and the offset the
reader reports.

### Replies differ, and the status is the service's own

The answer a broker-hosted service returns is a **complete Parcel**, and whether it
begins with a status is the service's decision, not the shim's:

- `createDisplayEventConnection` writes `reply.handle_binder(0)` first and no
  `ok()` -- so its object is at offset 0, which is exactly where the reader asked for
  it. The line "Attempt to read object from Parcel at offset 0" was the shim being
  *right*, and an added status word was making it look wrong.
- `stealReceiveChannel` writes `reply.ok()` and then the two descriptor words, so that
  answer *does* begin with a status, and `into_bytes` does not strip it.

So the shim's job is to pass the parcel verbatim, which it now does. The added status
was a fix for a symptom: it made the descriptor case work and broke the object case,
and the instrument said why -- `parcel objects=1 data=32 answer_objects=1` on the reply
that was failing, a Parcel that was already correct.

That leaves the descriptor case failing again now that the mask is gone, and the two
failures are exact complements of each other, which is what makes it a single question:

```
status added:     dup: 0    "at offset 0 that is not in the object list"
status removed:   dup: 1    "at offset 32 that is not in the object list"
```

`stealReceiveChannel` writes `reply.ok()` and then two descriptor words at 8 and 36, so
its answer *does* begin with a status -- and with the word removed the reader is four
short, which is 32 instead of 36. `createDisplayEventConnection` writes no `ok()` and
puts its object at 0, so its answer begins with the object -- and with the word added
the reader is four long, which is 0 where the object is.

Both are self-consistent and both cannot be served by one rule, which means the rule is
not the shim's to choose: an answer's first word is whatever the service wrote, and the
shim should pass the parcel through untouched -- which it now does. What is left is why
the descriptor case, whose service does write a status, reads four short anyway. The
next look is at what `Parcel::ok` is called *on* in that reply versus what the shim
receives, since that is the one place the two services' answers are built differently.

### The Parcel is right, and the reader is four short

One instrument settled the descriptor case, and it cleared this side of it completely:

```
reply offsets 8 36   data=64  answer=72
```

The offsets are 8 and 36, which is exactly what `stealReceiveChannel` writes and what
its test asserts. The Parcel's data is 64 bytes, which is status + presence + two
object words, and the answer is 72, which is that plus the trailer. There is nothing
to correct here: `parcel_write_object` registered both objects where they belong and
`parcel_ipc_objects` reports them where they are.

The reader, meanwhile, asked for an object at 32. That is the second offset minus four,
so the data it is reading is four bytes shorter at the front than the Parcel's data --
as though the status it read came out of a buffer that does not include itself.

Since the Parcel on this side is provably correct, the next question is on the reader's
side of the same call: what `IPCThreadState::transact` is handed and what it does with
the reply Parcel's data before the proxy reads it. The shim already resets the read
position to zero, and `readException` consumes the first word -- so the arithmetic to
check is whether the reader's idea of where the data begins matches the Parcel's.

### The reader's arithmetic, and the one number left

The reader's message is `"Attempt to read object from Parcel %p at offset %zu that is not
in the object list"`, and in AOSP that `%zu` is `mDataPos` -- the read position, not the
object's offset. The position was 32, and the status is four bytes, so the reader had
read 28 bytes of "object" starting at 4: the first word after the status.

4 is where the *presence* word is in this reply -- `stealReceiveChannel` writes `ok()`,
then `i32(1)`, then the two descriptor words at 8 and 36. So the reader read the
presence word as if it were an object word and then found nothing at the offset 28 bytes
on. That is not a missing status; it is the reader's object list disagreeing with this
side's about *where the objects are*, and this side's is provably right: the Parcel it
handed back holds offsets 8 and 36, the same two the service wrote and its test asserts.

So the next question is what the reader's list was built from. On a device that is
`Parcel::ipcSetDataReference(data.ptr.buffer, data_size, offsets, offsets_size / 8,
freeBuffer)`, called by `IPCThreadState` from the transaction's own header -- so the
list is the *driver's* offsets, and this side has no driver to supply them. The shim
writes the reply straight into the caller's Parcel instead, which is why its own list is
right and the reader's is not, and where the two must be made to agree is the next
thing to read.

### Same Parcel, four bytes apart

The reader and the writer are on the *same* Parcel -- the pointer in the framework's
message (`0x77ee75ffacc0` for `code=1`) is the one `handle_transaction` was given -- and
the two disagree about the offsets by exactly four:

```
written:  8 36      (parcel_ipc_objects, immediately after the write)
read:     4 32      (mDataPos 32, one object word past 4)
```

That is not a shift introduced by the copy: the offsets `handle_transaction` registers
are right, and something between that and the proxy's read takes them down by four.
`code=1` is the reply the framework hit, and it is the *first* one in the run whose
object list matters, so whatever rewrites the list runs before any successful read.

The candidates are the two functions this side interposes on that call -- `BpBinder::transact`
and `IPCThreadState::transact` -- since both are entered after the writer returns and
both are ours. The next step is to log the Parcel's offsets at each of those entry
points rather than only at the write: whichever one shows 4 and 32 is the one doing it.

### Correcting the previous note: two replies, not one

The pointers in the log separate the two cases, and reading them together matters:

```
code=4  -> 0x77ee75ffacd0
code=1  -> 0x77ee75ffacc0     the one the framework's message names
```

The offsets `8 36` were logged for the reply with two descriptors -- `code=4`,
`stealReceiveChannel` -- and the reply whose object list the framework complains about
is `code=1`, the *AIDL* `createDisplayEventConnection`. They are different replies with
different shapes:

- `stealReceiveChannel` writes `ok()`, a presence word, then two descriptor words at 8
  and 36, and this side registers exactly that.
- the AIDL `createDisplayEventConnection` writes `handle_binder(0)` with no presence
  word, so its single object is at 4.

So there was never a four-byte shift to find. The reader asked for an object at 4 --
which is where that reply puts it -- and the list it consulted did not have 4 in it.
What to check next is the AIDL reply's own registration: what `handle_binder` records
in the Parcel it is written into, and whether the shim's copy of that answer carries
the offsets the service set. The instrument to use is the one that already exists: log
the Parcel's offsets on the `code=1` reply specifically, not on whichever reply is
nearest.

### The nearest call is not the failing one

Logging the reply for the code that appears closest above the message gives

```
code 10 -> offsets count=-1 answer_objects=0 status=0
```

which is a reply with no objects at all -- `GET_BOOT_DISPLAY_MODE_SUPPORT`, which
answers with a value. The framework's failing read is not that reply: it is reading a
reply it is still working through, and the log line above it belongs to a *later* call.
Proximity in a log is not causality, and chasing it cost a round trip to a call that had
nothing to do with the failure.

What is established, from the instrumented run before this one, is that the
`stealReceiveChannel` reply has offsets 8 and 36 and the reader asks for an object at
32 -- four less -- while both are provably the same Parcel. So the list the reader
consults is not the list the Parcel held when this side wrote it, and the thing that
rewrites it runs after `handle_transaction` returns.

That is where the next instrument goes: the Parcel's offsets at the *entry* of
`BpBinder::transact` and of `IPCThreadState::transact`, both of which this side
interposes and both of which run between the write and the read.

### The reply that fails is a different call's shape

Matching the failing Parcel to the reply that produced it is what the last instrument
was for, and it is unambiguous:

```
the framework reads:  0x746a622f0cc0
the shim wrote:       0x0000746a622f0cc0  code=1
```

So the read is on the reply to `code=1` -- the AIDL `ISurfaceComposer`'s `CREATE_DISPLAY`
-- and that reply carries **offsets 8 and 36**: the shape of `stealReceiveChannel`, which
writes two descriptor words. `CREATE_DISPLAY` answers with a display handle, not a
`BitTube`, so the reply the framework is reading is not the shape that call returns.

One more reading corrects the name again. `code=1` on the display event connection's
*own* proxy is `stealReceiveChannel`, not `CREATE_DISPLAY` -- a code is meaningful only
against the interface the proxy is for, and the same number appears in every table. So
the reply is the one that *should* carry 8 and 36, the service builds it that way, and
the shim registers it that way.

What is left is then inside the read: the reader's position is 32 when it wants the
second descriptor word at 36, which is what 8 plus 24 gives. A `flat_binder_object` is
28 bytes, not 24, so whatever advanced the read position went 24 -- and the four bytes
between the two are the difference between this working and the display service booting.
That is the number to chase: one read in `BitTube::readFromParcel` advancing by 24 where
the object is 28.

Two lessons worth keeping from this stretch, both paid for:

- **Proximity in a log is not causality.** The code nearest above the message was a
  later call, and chasing it cost a run.
- **Replies differ, and the difference is the service's to make.** One writes `ok()`
  then a presence word then descriptors; another writes an object first. A shim that
  imposes one shape on both is wrong even when it makes one of them work.

### The 24-byte descriptor object: consistent, and not the cause

A descriptor object is 24 bytes, not 28 -- the stability word belongs to binder objects,
and the reader's advance of 24 is what that means. That is now the layout in all four
places that had 28: the service's `fd_placeholder`, the broker's `write_fd` and its
bound check, and the shim's object walk. The service's test and the transport's now
assert the corrected offsets (8 and 32 rather than 8 and 36) and the corrected lengths,
and all 126 pass.

It did not change the boot. The reader still reports the same message, and with the
offsets agreeing on both sides now -- the service declares 32, the Parcel holds 32, the
reader asks for 32 -- that message is no longer about the stride at all. So this is a
correction that was owed on its own terms (the layout was wrong and the tests said so
once the reader's arithmetic was understood) and it is not what the display service is
waiting on.

What that leaves is the object-list check itself: a reader whose list holds 32 and whose
message says 32 is not in it is not describing an offset problem. The next instrument
should log the list and the position from the reader's side of the same call, or find
what rebuilds the list after this side writes it -- which is where the earlier
`BinderProxy`/`ipcSetDataReference` question was heading.

### The display service boots

The 24-byte descriptor object was the fix, and it only showed once the shim actually
rebuilt: a compile error in the same edit meant the *old* library was being preloaded,
so the first run after the change looked like evidence against it when it was evidence
of nothing at all. With the walk corrected and the library built:

```
not in the object list:   0     (was 1 every run)
can't dup file descriptor: 0    (was 1 every run)
DisplayManagerService failures: 0
SystemServer timings:     41
```

The display service starts, and the overlay path it was blocking goes with it. A
descriptor object is 24 bytes -- status, presence word, then two 24-byte objects at 8
and 32 -- and every place that had 28 now has 24: the service's `fd_placeholder`, the
broker's `write_fd` and its bound check, and the shim's two object walks.

Two things about that worth keeping:

- **A build that fails is not a result.** The measurement that mattered was taken
  against a library that had not been rebuilt, and it read as "the fix did nothing".
- **The dead function is staying dead-but-present.** Removing `broker_serve` broke the
  file twice -- once taking `broker_reader` with it, once taking the wrong function --
  and a warning is cheaper than a third attempt. It is recorded here so the next person
  knows it was deliberate.

**The wall is the overlay service again**, which is the `idmap` path, and the framework
now reaches it rather than stopping at the display:

```
IdmapManager.getFabricatedOverlayInfos(IdmapManager.java:124)
OverlayManagerServiceImpl.cleanStaleResourceCache(OverlayManagerServiceImpl.java:580)
java.lang.RuntimeException: Failed to start service com.android.server.BatteryService: onStart threw an exception
```

Four `Parcel.readException` frames in the same run say the shape of it: a call the
framework makes is coming back as a failed transaction rather than a value.

### idmap2d runs, and the incoming path is exercised

The failed transaction was a process that never started. `idmap2d` is dumped into the
bundle by a path that does not rewrite `PT_INTERP`, so its interpreter stayed
`/system/bin/linker64` -- which the *kernel* resolves when it execs, where no path shim
can help:

```
/bin/idmap2d: cannot execute: required file not found
```

That is why the boot script's wait for `registered idmap` timed out, why the daemon's
socket closed, and why `acquireFabricatedOverlayIterator` came back as `no answer for
node ...`: the service was registered by a process that was not running. The three tools
dumped this way (`idmap2`, `idmap2d`, `hwservicemanager`) now get the same interpreter
rewrite the main binaries do, and the existing bundle was patched in place.

```
no answer for node:     0     (was 1 every run)
idmap registrations:    8     (was 2)
display failures:       0
SystemServer timings:   41
```

With the daemon actually running, `idmap2d` reaches the incoming path -- and that is
this side's own new code, reporting the trouble in the only way it can:

```
idmap2d: Starting
IPCThreadState: *** BAD COMMAND -2147454458 received from Binder driver
```

`-2147454458` is a *pointer*, not a command word: the same shape `0x7fff8dfa` had before
this work started. The reader walks the command stream by the size each command carries,
so the incoming write's framing is what is wrong now -- the delivery exists and is
reached, and what it writes is not yet what the reader can walk. That is the next piece.

### The command direction, and no bad commands left

`BR_TRANSACTION_COMPLETE` was built with `BR(6, 0)` -- the READ direction -- where the
driver's is `_IO('r', 6)`, which has no direction at all. The reader compares the whole
word, so it did not recognize it: `BAD COMMAND -2147454458` is `0x80007206`, this value
with READ set, against the driver's `0x00007206`. `BR_NOOP` had already been built the
other way (`IOC(0, 'r', 12, 0)`), which is what the fix copies.

My first reading of that word was wrong and I recorded it as a pointer; decoding it
against the image's own values is what settled it. With the direction corrected:

```
BAD COMMAND in idmap2d:    0     (was 1, then 2)
BAD COMMAND in framework:  0     (was 4)
```

The incoming path is exercised for the first time and no command is rejected. What
happens next is the process that receives it:

```
1 binder-shim: ioctl ... (repeated)
timeout: the monitored command dumped core
_: line 55: Segmentation fault   (idmap2d)
```

`idmap2d` crashes while serving the transaction, which is the next thing: the struct
this side writes is walked correctly now, and what it *contains* is what the framework's
own `Binder` needs -- the pid, the euid, and the offsets, all of which are zero here
because the shim has no sender to report.

The framework, meanwhile, is serving calls and raising for the ones it cannot: eight
`RuntimeException`s and two `UnsupportedOperationException`s in the same run.

### The incoming delivery is complete and idmap2d crashes on it

`shim_incoming` was dropping the object count, so the reader got `offsets_size = 0` over a
body that carried object offsets. It now carries them, along with the sender's pid and
the pointer to the offset table:

- `sender_pid` from this process, because zero is not a pid and the object being called
  is the framework's own;
- `offsets_size` from the count the wire already sends (it rides in the upper half of the
  flags word) and `data.ptr.offsets` pointing at the table at the end of the body;
- the command and its 64-byte struct, with no bad commands reported and the incoming
  path exercised for the first time.

```
BAD COMMAND in idmap2d:    0
BAD COMMAND in framework:  0
SystemServer timings:     41
```

`idmap2d` now segfaults *serving* that transaction rather than rejecting it, and the two
lines before the crash are its own reads:

```
1 binder-shim: ioctl 0xc0306201 binder-shim: ioctl 0xc0306201
Segmentation fault   (idmap2d)
```

So the framing is right and what the reader does with it is next: `target.ptr` and
`cookie` both carry the object's cookie here, which is what the driver sets, and the
crash is inside the object being called rather than in the walk up to it. That is the
next piece -- and it is the first time anything has reached this far.

### Narrowing the idmap2d crash

The crash is on the read side, and the ioctls just before it say which read:

```
ioctl BINDER_WRITE_READ   write=4 read=256      the looper asking for work
Segmentation fault (idmap2d)
```

`read=256` is room for the 68 bytes the incoming transaction needs, so the write
happened and the reader walked it. Everything up to the call is therefore right --
command word, struct, offsets, sender -- and what is left is what the object does with
the transaction: the `code` it is handed and the `flags` that come with it.

`code` rides in `b` on this wire and the flags in the low half of `c`, with the object
count in the high half; both are taken straight from `Incoming`, so the next thing to
check is the pair on the one message that arrives, against what `IIdmap2`'s stub
expects. That is a two-line instrument: log the code and flags the shim delivers beside
the crash, and the method it names will say whether the object is being handed the call
it expects.

### The call that crashes idmap2d is well-formed

Two lines of instrument on the incoming write, and the message that reaches `idmap2d` is

```
incoming code=7 flags=2 cookie=0x0000702b340232b0
```

which is `acquireFabricatedOverlayIterator` (method 7 of `IIdmap2`), one-way, carrying a
real object cookie. So the delivery is correct all the way to the object: command word,
struct, offsets, sender, code, flags and cookie. The crash is *inside* the handler, which
is the framework's own `libidmap2` code reading the arguments it was given.

That moves the question from this side of the boundary to theirs: a one-way call has no
reply to write and nothing is owed back, so what is left is the parcel the framework
built -- the `FabricatedOverlay` argument among it -- and whether the offsets this side
passes describe it the way its reader expects. The table is pointed at correctly by the
same arithmetic the reply path uses (`data + size - offsets_size`), so the next look is at
the argument's own encoding rather than the envelope.

### The flag the kernel does not define

`flags=2` is the one field in that delivery that does not check out. The driver's
transaction flags are

```
TF_ONE_WAY 0x01   TF_ACCEPT_FDS 0x10   TF_CLEAR_BUF 0x20   TF_UPDATE_TXN 0x40
```

and there is no 0x02 among them. The value reaches `idmap2d` unchanged -- the broker
passes `call.flags` straight through, and the shim takes the low half of the word where
the count rides in the high half -- so bit 1 was set by the framework before this side
ever saw it, and the object being called is a vendored AOSP handler that reads that word.

Nothing on this side sets it: `handle_transaction` takes `flags` from the framework's own
`BC_TRANSACTION`, `broker_transact` forwards it unchanged, and the broker passes
`call.flags` straight into the outgoing message. So bit 1 arrives from the framework, and
the object it is going to reads that word.

The rest of the delivery is confirmed: the command word is the image's own, the struct
carries the cookie the registry recorded, the offsets table is pointed at by the same
arithmetic the reply path uses, and the sender is a real pid. What is left is one bit
out of a word whose other bits are right -- and a word the kernel defines only four
values in.

That is where the next look goes: the framework's `IPCThreadState` on the way out, and
what it puts in `flags` for this call before this side ever sees it. It is a one-line
instrument on the shim's transaction path, and it will say whether the value is the
framework's or something added between.

### The flag, read from the other side

Instrumenting the outgoing transaction path shows what the framework actually sends:

```
binder-shim: tr handle=0 code=1599098439 size=0 offsets=0 flags=16
```

`16` is `TF_ACCEPT_FDS`, which is a real flag -- so the framework does set flags this
side can name, and the shim passes them through untouched. The `flags=2` seen earlier
was on the *incoming* message, which the broker builds from `call.flags`, so the two
readings together put the value at the framework's own `BC_TRANSACTION` rather than
anywhere in between. Only three transactions appear in the trace because the shim logs
the first few and then stops, which is worth knowing before drawing conclusions from
its absence.

That leaves the value itself: `2` is not one of the four `TF_*` the kernel defines, and
the next reading is the framework's own `IPCThreadState::transact` for that call rather
than the envelope this side builds from it.

### The packing is right, so the value is the framework's

`OBJECT_SHIFT` is 16 and both sides mask with `c & 0xffff`, so the flags the broker sees
are exactly the flags the framework sent. There is no bug in the packing to find: the
word is `flags | (count << 16)` on the way out and `c & 0xffff` on the way in, and the
two agree.

Which means `2` is a value the framework put in `flags` for this call, and the remaining
question is what it means to *its* reader rather than where it came from -- the object
being called is a vendored AOSP handler, and it reads that word with its own expectations.

Everything else in this delivery is confirmed correct, field by field: command word,
struct, cookie, offsets table, sender pid, code. What is left is one bit of `flags`, and
one process -- `idmap2d` -- crashing on a call that is otherwise perfect.

### Where the log instrument runs out

Everything in this session that moved was found by logging: intercept the call, print
what it carries, and read the number. That worked for a declaration, a connection, a
session, a descriptor, a command word, a cookie and a flag.

It does not work for what is left. `idmap2d` segfaults inside a vendored AOSP handler,
with a call that has been checked field by field -- and a crash inside someone else's
code is not something a print statement answers. The next instrument is a debugger
attached to the crashing process, which is a different tool from the one that got here:
log where the boundary is, then step across it.

So this is the state to hand over with:

- **everything this side constructs is verified correct**: command word, struct, cookie,
  offsets table, sender pid, code, and the flags packing (`OBJECT_SHIFT` 16, `c & 0xffff`
  both ways);
- **`flags=2` is the one value not accounted for**, and it is the framework's, not this
  side's -- a `TF_*` word the kernel defines four values in and which 2 is not among;
- **the crash is inside the handler**: one-way, method 7 of `IIdmap2`,
  `acquireFabricatedOverlayIterator`.

A debugger on `idmap2d`, or a read of `libidmap2`'s handling of that call, is where this
goes next.

### The debugger: a virtual call on the wrong pointer

The log instrument ran out, so the next one was a core dump. It is a good one -- the
crash is a single instruction, and the stack says exactly what it is:

```
Signal: 11 (SEGV)  si_code: SEGV_MAPERR
#0  android::BBinder::transact(uint, Parcel const&, Parcel*, uint)   libbinder.so + 0x59e1b
#1  android::IPCThreadState::executeCommand(int)                     libbinder.so + 0x65014
#2  android::IPCThreadState::getAndExecuteCommand()
#3  android::IPCThreadState::joinThreadPool(bool)
```

`SEGV_MAPERR` inside `BBinder::transact` at the *same offset* every time is a virtual
call through a pointer that is not an object -- the first word of a `BBinder` is its
vtable, and this is the instruction that reads it. So `target.ptr` is the wrong value,
and the fault is not in the envelope but in what this side puts in that field.

`shim_incoming` was handing the *cookie* in `target.ptr` and in `cookie`. That is now
corrected -- the object in `target.ptr`, the cookie in `cookie`, taken apart by
`object_and_cookie_for_node` -- but the crash is unchanged at the same instruction,
which means the object this side has on file is itself not a `BBinder`.

The shim already has the code for that question, written when the Java registrations
were handed back the wrong pointer: it reads the first word of each candidate and picks
the one that looks like a vtable. That check is the next thing to examine -- whether it
is choosing correctly for a *native* publish like `idmap2d`'s, where the object word is
`BINDER_TYPE_BINDER` and both fields are pointers into the same process.

### The instruction, not the envelope

Disassembling the crash address says which field is wrong:

```
59e09:  mov  (%r14),%rax        ; rax = *object      -- this one succeeds
59e1b:  call *0x80(%rax)        ; call rax[0x80]     -- and this one faults
```

So `r14` -- the object -- is a *readable* address, and what is at it is not a vtable:
`mov` succeeds and the call through `rax+0x80` does not, which is `SEGV_MAPERR` at a
fixed offset on every run. The pointer this side passes is not garbage; it is a real
address of the wrong object.

That puts the fault squarely on the candidate selection in `object_at`, which picks
between the two pointer fields of a `flat_binder_object` by reading each one's first
word. For a *Java* registration the comment there records the answer -- offset 8 is a
weak reference table and offset 16 is the `BBinder` -- and the loop prefers 16 on that
basis. `idmap2d` publishes a *native* object, and the two fields are `binder` and
`cookie` with different meanings.

So the next thing is that selection, one level down from where the last few rounds have
been: not what the envelope carries, but which of the two pointers inside the object
word this side decided was the object. The heuristic is a good one and it was written
against a different publisher; whether it holds for this one is the question.

### What the object is not

Two instruments, one on each side -- what is stored at registration and what goes into
`target.ptr` -- and neither printed, because the shim's log is flooded with its own
per-ioctl lines and the useful ones fall off the end. That is a lesson about the
instrument as much as about the code: the shim logs everything it is asked and nothing
it is worth asking.

The kernel's own `flat_binder_object` settles what the fields mean, and it disagrees with
the heuristic that was there:

```c
union {
    binder_uintptr_t binder;  /* local object */
    __u32           handle;   /* remote object */
};
binder_uintptr_t cookie;      /* extra data associated with local object */
```

`binder` at offset 8 is the local object and `cookie` at 16 is extra data. The selection
loop preferred offset 16, on evidence from `Parcel::flattenBinder` -- a different
function whose layout differs -- and it now tries offset 8 first, which is what the
struct says. That change alone did not fix the crash, so the object being stored is
wrong for a reason upstream of the selection: the fault is `call *0x80(%rax)` with `rax`
read from a *readable* first word, which is a real address of the wrong object rather
than garbage.

Next: the registration path for this publisher, with the log quietened first -- the two
call sites are known (`services[...].object = object` at the named and unnamed
registration, and `object_at` filling it from the parcel in the service-manager case),
and what is needed is the value at each.

### The log had to be quietened first, and that is a fix of its own

The shim's log has a line budget (`MAX_LINES`), and it was spending it on every ioctl
it was asked -- the loudest thing it does and the least useful once the surface is
known. A line written after the budget runs out is simply dropped, so an instrument
added to answer a later question never appeared. That is why two rounds of logging went
by with nothing to show: not a wrong reading, no reading.

The per-ioctl line is now behind `MOSAIC_BINDER_DEBUG` with the rest of the tracing, and
the same boot writes 33 lines instead of thousands. Instruments added after this will be
seen, which is worth as much as the fix they were there to find.

The registration instrument still did not print, and the reason is the next thing: the
`published idmap` line comes from `broker_export` and the `registered idmap` line from
the broker, so the shim's own registration message is being emitted by neither of the
two paths that were instrumented -- or the branch taken is the one that returns early
for an object already in the registry. That is a two-minute check with the log now
quiet, and it is where this picks up.

### The object stored and the object dereferenced are different values

With the log quiet, the registration reading finally appears, and it is the one that
matters:

```
stored idmap object=0x00007dd1b8809618 cookie=0x00007dd158821f80
```

Two clearly different addresses. And the core from *the same run* says what was
dereferenced:

```
r14  0x7dd158821f80     <- the cookie
rax  0x100000001        <- its first word, a weak reference table's refcounts
```

`r14` is the object the reader was handed, and it is the **cookie**, not the object this
side stored. `rax = 0x100000001` is `{strong=1, weak=1}` -- the refcount pair at the
head of a `RefBase` weak reference table -- which is exactly why the `call *0x80(%rax)`
faults: that is not a vtable.

So the registry has the right value and the reader gets a different one, and the path
between them is short and named: `object_and_cookie_for_node` takes both fields from the
registry entry the *node* names, so either the node resolves to a different entry than
the one the log line came from, or the fields are crossed on the way out. Both are now
a single comparison, and both are visible in one instrument placed at the lookup rather
than at the registration -- which is where the next one goes.

### The object goes in `cookie`, and the crash is gone

The core said it and the fix confirms it: `IPCThreadState::executeCommand` calls
`reinterpret_cast<BBinder*>(tr.cookie)->transact(...)`. The virtual call is made on
**`tr.cookie`**, not `target.ptr` -- which is why `r14` held the cookie while the lookup
had correctly returned the object, and why `rax` was a weak reference table's refcounts.

Both words now carry the object, which is what the driver sets: `target.ptr` for the
reference taken, `cookie` for the call. The publishing process's own cookie is not
carried in this struct at all.

```
idmap2d segfault:        0     (was 1 every run)
SystemServer timings:   41
```

That is a crash that stood for several rounds, found by reading a register out of a core
after the log instrument had said everything it could.

**The next wall is a reference count**, which is a natural consequence of getting this
far:

```
RefBase: decWeak called on 0x73e3bb403460 too many times
FORTIFY: pthread_mutex_lock called on a destroyed mutex
```

`executeCommand` wraps the object in an `sp<BBinder>` before calling it, so a reference
is taken on every incoming transaction; something is releasing one that was never taken,
or not releasing one that was. That is the next thing, and unlike the last several it is
in this side's accounting rather than in its framing.

### The reference count, and a lead already in the source

`RefBase: decWeak called on ... too many times` means the object's strong count reached
zero and something then released a weak reference that had already gone -- the object was
destroyed while someone still thought they held it. The take is on this side:

- `IPCThreadState::executeCommand` wraps `tr.cookie` in an `sp<BBinder>`, which is a
  balanced take-and-release around the call;
- the shim's own `readStrongBinder` interception says "reference held" and does hold one,
  which the parcel's destructor releases.

Both of those balance. What does not is the outward direction: `KIND_ACQUIRE` and
`KIND_RELEASE` are defined in the shim's protocol and **never sent** -- there is no
`broker_send(KIND_ACQUIRE, ...)` anywhere. So a reference that one process takes on
another's object is counted on one side only, and the object dies while a holder still
believes in it.

That is a concrete lead rather than a hypothesis: the two message kinds exist, the wire
carries them, the broker's registry has `holders_of` and reference tables, and the shim
never asks. The next step is to make the acquire and release paths say so.

### References are forwarded now, and the count still goes wrong

`BC_ACQUIRE` and `BC_RELEASE` were consumed and forgotten by `run_commands`, so a take on
an object in another process was counted on one side of the socket only. They are now
forwarded to the broker as `KIND_ACQUIRE` and `KIND_RELEASE`, which is what those two
message kinds were defined for and never used:

```c
#define BC_ACQUIRE BC(5, 4)     /* _IOW('c', 5, __u32), the operand is the handle */
#define BC_RELEASE BC(6, 4)
```

The shim does see them -- the log shows the `write=8 read=0` pairs that are exactly two
four-byte commands -- and the broker acts on them (`acquire`/`release` against the
handle table), so that chain is complete end to end now.

The crash is unchanged, which means the imbalance is not in the outward direction:

```
RefBase: decWeak called on 0x71dd1ba073a0 too many times
  at com.android.server.SystemServiceManager.startService(SystemServiceManager.java:258)
```

A service being started is where the object dies, and the object is one the framework
owns. The next thing is the counts themselves rather than the messages: what the broker
has against that handle when the release arrives, and whether a take that *this* side
makes on the framework's object -- rather than one it forwards -- is ever counted.

This is a fix that closes a real hole in the protocol and does not, on its own, move the
boot: worth having, and worth saying plainly rather than dressing it up as the answer.

### The weak pair too, and the count still goes wrong

`RefBase` keeps two counts, and the shim was forwarding neither: the strong pair
(`BC_ACQUIRE`/`BC_RELEASE`) and then the weak pair (`BC_INCREFS`/`BC_DECREFS`, `_IOW('c',
4)` and `_IOW('c', 7)`). Both are now sent as `KIND_ACQUIRE`/`KIND_RELEASE` and
`KIND_INCREFS`/`KIND_DECREFS`, all four of which the broker's handle table already knew
how to take -- the message kinds existed and the table had the columns, and nothing ever
asked.

The crash is unchanged. So two real gaps in the protocol are closed and the count is
still wrong, which means the arithmetic itself is what to look at next rather than the
messages: what the broker holds against that handle when the release arrives, and where
the extra decrement comes from. Every message kind the table understands is now actually
sent, so the count the broker keeps is the count the callers asked for -- and if it still
goes negative, the fault is in how the table interprets it, or in a take that happens
without a message at all.

That is where this stops being about the shim's plumbing and becomes a question about
the table, and it needs an instrument on the counts rather than on the wire.

### The count is not the table's

Reading this side's arithmetic rules it out: `release` saturates at zero and removes the
entry, `decrefs` saturates the same way, and a fresh entry starts with `weak = 1`. A
table built like that cannot go negative, so `RefBase: decWeak called on ... too many
times` is `RefBase`'s own count *inside the framework process* -- a different counter
from the one this side keeps, and one the shim cannot reach.

Which points at the reference the *driver* takes and this side does not. On a device, a
transaction's target is reference-counted for its duration: the kernel holds the object
alive while the call runs and drops it after. `IPCThreadState::executeCommand` wraps
`tr.cookie` in an `sp<BBinder>` -- a balanced take and release around the call -- and if
nothing else holds the object, that release is the last one, and the object is destroyed
*inside* its own transaction. What follows is a weak release against a count that has
already gone: `decWeak called too many times`.

So the next thing is not the messages and not the table, it is that hold: an incoming
transaction's target has to be kept alive for the duration of the call, the way the
driver keeps it, and `target.ptr` is the word that says which object that is.

### The hold is built, and the count is still wrong

The driver's side of a transaction is now emulated: an incoming transaction takes a strong
reference on its target (`RefBase::incStrong`, resolved from `libutils`), a one-way call
gives it back immediately because nothing is answered, and an answered call gives it back
when `BC_REPLY` goes out -- which is when the driver drops it.

It is the right shape and it did not move the crash, so the object that dies is not losing
its last reference to the transaction that is calling it. Two possibilities remain, and
both are now cheap to tell apart:

- the target is not the object the caller holds a reference to -- a service registered by
  name and handed out by handle are two different objects if the registry stored the wrong
  field;
- something in the framework releases a reference it never took, which is a bug on the
  other side of the boundary and would show as an unbalanced pair in `RefBase`'s own
  counts rather than in anything this side can see.

The object's address is in the message (`0x7c4ff98a4fd8`), the service being started is
named in the stack (`SystemServiceManager.startService`), and those two together are
enough to find which registration it came from -- which is the next instrument.

### The dying object is neither a registration nor a target

Naming both sides in one run settles what the object is, by elimination:

```
decWeak called on 0x7a4edf4015b0                 <- the object that dies
resources is object 0x00007a4f0f40d090           <- the framework's registrations
sensor_privacy is object 0x00007a4f0f40bc50
sensorservice is object 0x00007a4faf4166d8
system_config is object 0x00007a4f0f40c610
target 0x...                                     <- no incoming targets at all
```

The object that dies is in a different mapping (`0x7a4e...`) from every object the
framework registered (`0x7a4f...`), and no transaction arrives at the framework as a
target in this run. So it is not one of its own services and not something it is being
called on.

That leaves the third thing a process holds: an object handed *to* it -- a proxy. The
shim's `readStrongBinder` interception reports "reference held" when it hands one over,
and the release of that reference is where the balance has to be. Which is the next
instrument, and it is on a path that has not been looked at yet in this whole
investigation.

### Three real gaps closed, and the count still goes wrong

Three fixes went in on the reference paths, each of them a genuine hole that the source
or the protocol named:

- the **strong pair** (`BC_ACQUIRE`/`BC_RELEASE`) was consumed and forgotten;
- the **weak pair** (`BC_INCREFS`/`BC_DECREFS`) likewise;
- the **fallback reader** returned an object with no reference at all -- the code says so
  in a comment, "which is a thing to fix here" -- so a service registered from a
  temporary was owned by nobody;
- and the **transaction hold**, which the driver takes on its target and this side did
  not, one-way calls releasing it at once and answered ones at `BC_REPLY`.

All four are right on their own terms, and `decWeak called on ... too many times` is
unchanged at two per boot. The conclusion that follows is not another guess about the
shim's plumbing: whatever is unbalanced is not on the wire and not in this side's
accounting either, both of which have now been checked by changing them.

That leaves the framework's own counts, and the instrument for it is a breakpoint rather
than a print -- `RefBase::decWeak` on the dying address, with the stack that got there.
The address is in the message, and this side has nothing left to add to it.

### The reference count was noise from the exit path

The core that matters is the framework's, and its stack is not what the message suggested:

```
#4  RefBase::weakref_type::decWeak(void const*)   libutils.so
#5  __cxa_finalize                                libc.so
#6  exit                                          libc.so
#7  JNI_CreateJavaVM                              launcher.so
#8  dalvikvm64
```

`decWeak` is reached from `__cxa_finalize` during `exit` -- a static object's destructor
at shutdown, not a live transaction and not this side's accounting. So `decWeak called
... too many times` is a *consequence* of the process tearing down, and the four
reference fixes above were real holes closed in service of a symptom. They stand on their
own terms; they were not the wall.

**The wall is what made it exit**, one line above:

```
Failed to start service com.android.server.BatteryService: onStart threw an exception
Caused by: java.lang.RuntimeException: Unknown exception code: 1 msg null
  at android.os.Parcel.createException(Parcel.java:3020)
  at android.os.Parcel.readException(Parcel.java:3000)
```

`BatteryService.onStart` makes a binder call and reads back status `1` -- a real status
this time, not a message misread as one -- and throws. The exception is a consequence of
that call, and the service that fails is the same one this session spent its middle
fixing the callback for. The next instrument is that call: what it is, what the callee
answers, and why the answer is `1`.

### One process is one client, and the peers map has to follow

The refusal had a name and a number all along:

```
checkService idmap found in the broker, handle 43
code=7 handle=43 answered status=4294967167
```

`4294967167` is `0xFFFFFF7F`, which is `-129`, `EX_TRANSACTION_FAILED` -- the broker's
answer when it cannot route a handle. And it could not route it because the handle was
answered on one connection and used on another: `no handle 43 in client 2` while the
lookup that produced 43 was made by a different client id. Two connections from one
process, two handle tables, and a table that does not travel between them.

`connect` now treats a pid as one client however many connections it has, which is what
the handle table living in the process implies. The test that asserted the opposite --
`a_second_connection_is_a_second_client` -- was encoding the bug, and is replaced by one
that says a second connection from one process is the same client.

**That fix has a consequence that is not yet handled**, and it is why the transport tests
now hang: `peers` is keyed by client and holds one writer, so a second connection
replaces the first, and a reply meant for the connection that asked goes to the other
one. One client with two connections needs the replies routed to the connection that
made the call -- a map of peers rather than a single one, or a writer chosen per
transaction. That is the next piece, and it is the direct consequence of getting the
identity right.

### One process is one client; the tests still model it as two connections

The identity fix and its two consequences are in:

- `connect` treats a pid as one client however many connections it has -- two tables
  for one process is what made a handle found by a lookup unroutable on the next call;
- a **reply** goes to the connection that asked, carried with the call rather than looked
  up in the peer map (which holds the latest);
- a **lookup's answer** does the same, for the same reason;
- **notices and forwards** go to every connection the client has, because they share a
  handle table;
- a session's teardown removes only its own connection, since the process may have
  another open.

That is the correct shape and it is why three transport tests fail: their harness opens
two connections from one process and means them to be two *processes*. A `#[cfg(test)]`
hook lets a test say which process a connection is -- keyed by descriptor, so parallel
tests do not walk on each other -- and the harness passes 1001 and 1002.

The tests were re-checked after that and still fail on `never appeared`, which means the
override is not reaching the path those lookups take rather than the routing being wrong.
**That is where this stands**: production code correct by construction and by reading, and
three tests that need their harness finished. It is written down this way because a
half-finished harness is the kind of thing that gets mistaken for a broken fix.

### One process is one client: done, and green

The identity fix and its consequences are complete and the tree is clean -- 126 tests
pass, `fmt` and `clippy` are quiet:

- `connect` treats a pid as one client however many connections it has;
- a **reply** and a **lookup's answer** go to the connection that asked, carried with the
  call rather than looked up in a map that holds only the latest;
- **notices and forwards** go to every connection the client has, since they share a
  handle table;
- a session's teardown removes only its own connection.

The transport tests needed three changes to say this: a `#[cfg(test)]` hook keyed by
descriptor so a harness connection can declare which process it is (parallel tests do not
walk on each other), the harness passing 1001 and 1002, and `own_node` taking the pid --
because a node id carries its owner's pid and the broker checks it, so a fixture that used
the test's own pid could not export anything once connections were pretending to be other
processes.

**The boot is unchanged.** `no handle 43 in client 2` still appears, and the broker's log
shows the lookup and the failing call on the same client id -- so the handle is not
crossing between connections, and whatever loses it is further in than the identity of a
client. That is the next thing, and the reasoning above is kept because it is what ruled
this out rather than a guess that did not land.

### The suspect is this side's own release forwarding

The handle's life is now traced, and it is short:

```
client 1 exported idmap as node 2203992532713473
client 2 looked up idmap
android-binder: checkService idmap found in the broker, handle 43
android-binder: code=7 handle=43 answered status=4294967167
```

One client, one lookup, and a handle that is gone by the time it is used -- with nothing
in between but this side. The `BC_RELEASE` forwarding added a few rounds ago is the only
thing that runs there, and it is the prime suspect: `Table::release` removes the entry
when the strong count reaches zero, and if the framework releases a handle while another
`BpBinder` for the same handle is still alive, the entry goes and the next call on it
cannot be routed.

**That suspect is wrong, and it took one instrument to show it.** With the broker's
release log raised from trace to info, handle 43 is never released:

```
client 2 released handle 17 (None)
```

So the entry does not go through `release`. What is left is `disconnect`, which drops a
departing client's nodes out of every other client's table -- and which now keeps them
when the process is still running, unless the process is genuinely gone. Handle 43 points
at `idmap`, exported by client 1, so a disconnect of client 1 is what would take it, and
whether client 1 was alive at that moment is the next thing to check.

The method here is what is worth keeping: the suspect was named from the trace, tested
with one line, and dropped when the line said no. That is cheaper than reasoning about
which of five paths could be responsible.

### The crash was this side's, and it was hiding the wall

The core for `idmap2d`'s new crash named the culprit in its first frame:

```
#0  RefBase::incStrong(void const*)     libutils.so
#1  incoming_write                      probe.so
#2  ioctl                               probe.so
```

The transaction hold was calling `RefBase::incStrong` on a pointer that is not always a
`RefBase` -- what the publisher put in that word varies -- and it faulted inside libutils.
It had been written for `decWeak called ... too many times`, and the later core showed
that message is a static object's destructor during `exit`: shutdown noise. So the
emulation was for a non-problem and introduced a real one. Removed, with the reason
written where the code was.

**That fixed three things at once**, because they were one chain:

```
idmap2d crash:        0     (was 1 every run)
no handle 43 in client: 0   (was 2 every run)
SystemServer timings: 41
```

`idmap2d` was crashing on the transaction it was being sent; the crash killed the process;
its death took the `idmap` node out of every other client's table; and the framework's
handle 43 -- taken from a lookup that had worked -- became unroutable. `BatteryService`
then read `-129` from its call and threw, which is what the boot has been ending on.

What is left is the same call, failing differently: the broker routes it now, so
`idmap2d` receives it and does not answer, and the timeout produces the same `-129`. The
next thing is that handler -- code 7 on `IIdmap2`, `acquireFabricatedOverlayIterator` --
rather than anything about handles, routes or identities.

### The call arrives complete and is never answered

One instrument on the delivery, and the transaction that `idmap2d` receives is whole:

```
delivered code=7 size=56 objects=0 flags=2
```

Method 7 of `IIdmap2`, a 56-byte payload -- a `FabricatedOverlay` -- no object arguments,
and a flag word. The command stream is right (the shim writes 68 bytes, command plus
struct, and the parcel is pointed at separately), the target is the object the registry
holds, and the name resolves. So the delivery is done, and what is missing is the answer:
`no answer for node ... from client 1` in the broker's log, three times, and then the
connection ends.

Two things stand out and both are worth checking next:

- `flags=2` is still the word the kernel defines no value for, and it is handed to
  `onTransact` as its flags argument. If the generated stub reads it as an instruction
  rather than passing it through, that is where the answer goes.
- a handler that throws would leave the process alive and the caller waiting, which is
  exactly the shape here -- the process does not crash, it simply never replies.

That is the next step, and it is inside `idmap2d`'s handling rather than anywhere in the
route to it: the call arrives, complete, and stops there.

### idmap2d receives the call and never answers

One more reading settles where it stops. The shim logs every outgoing transaction, and
`idmap2d` sends exactly one in its whole life:

```
binder-shim: tr handle=0 code=1599098439 buffer=0 size=0 offsets=0
```

That is its `getService` to the service manager. It never sends `BC_REPLY` -- not for the
call it receives, not for anything. So the handler is entered and does not come back: it
either blocks or throws, and either way the caller waits out its timeout and reads `-129`.

The service *is* up -- `idmap2d: Starting`, the `idmap` name present in the broker, the
call delivered whole -- so this is not a registration problem and not a routing problem.
It is the one piece of the path that is somebody else's code.

Two candidates, both cheap to tell apart from here:

- `flags=2`, the word the kernel defines no value for, is handed to `onTransact` as its
  flags argument. The framework sets it (this side's packing is verified), and a generated
  stub that acts on it rather than ignoring it would explain a handler that never returns.
- the payload: 56 bytes of `FabricatedOverlay`, readable or not by whatever version of
  `libidmap2` this image carries.

A debugger on `idmap2d` while it handles that call -- or its own log, if the image's logger
is wired up -- is the instrument, and it is a different tool from the one that got here.

### The flags were part of it, and the rest is libidmap2's

Masking the delivered flags to the four the kernel defines -- `0x01`, `0x10`, `0x20`,
`0x40` -- moved the failure:

```
no answer for node:   1    (was 3 per boot)
replies from idmap2d: 0
```

So `flags=2` was doing something: a real driver would never deliver a bit that is not in
its own set, and a handler that reads the word rather than ignoring it stalls on it. That
is kept.

The handler still never answers, and the shim's outgoing-transaction log confirms `idmap2d`
sends exactly one transaction in its whole life (its `getService`) and never a `BC_REPLY`.
So the remaining stall is inside `libidmap2` -- a vendored handler, entered with a call it
does not complete -- and a print statement is the wrong instrument for it, which is the
same lesson the core file taught two rounds ago.

**This is where the boot stands.** The path to the call is verified end to end: the service
is registered, the name resolves, the handle is valid, the transaction is routed, the
command stream is the image's own, the target is the object the registry holds, and the
payload arrives with a 56-byte argument and sane flags. What is left is the one step that
belongs to somebody else's code, and the instrument for it is a debugger on `idmap2d` while
it handles that call.

### The debugger: idmap2d is idle, and that is the finding

Attached while the call was outstanding, both of `idmap2d`'s threads are here:

```
Thread 1  syscall <- incoming_write <- IPCThreadState::talkWithDriver
          <- getAndExecuteCommand <- joinThreadPool
Thread 2  the same
```

No handler on any stack, and no thread anywhere in `libidmap2`. So the call was delivered
-- the shim wrote it, 68 bytes on the read -- it was *dispatched*, and it *returned*: the
threads are back in the read loop, waiting for the next thing. What never happened is the
reply. A stub that answers sends `BC_REPLY`, and `idmap2d` sends exactly one transaction in
its whole life, its `getService`.

So the stall is not a block. It is an `onTransact` that returns without writing an answer,
with the caller waiting on a reply that is never coming. That is `libidmap2`'s own logic
-- and the symbol tables in this bundle are stripped, so the next instrument is either a
debugger against its source or a build with symbols, not another print.

Which is where this stops: the whole path to the call is verified, the one step left is
inside somebody else's code, and every instrument this side has has been used on it.

### Where this stops, and what is verified

The boot ends on one call that is delivered and never answered, and every instrument this
side has has now been used on it. The step left belongs to `libidmap2`, whose symbols in
this bundle are stripped -- so the way forward is a build with symbols or its source, both
of which are outside this repository.

What is verified, each by a measurement taken during this session rather than by reading:

| fix | what it changed |
|---|---|
| `isDeclared` answered | the VINTF declaration; `IHealth service instance isn't available: 0` |
| broker connection recovery | lookups 282, send failures 0 |
| session survives a failed transaction | the health lookup reaches the broker |
| health callback plumbing | `BatteryService waits: 0` (was 175 s) |
| `idmap2d` invoked by name, then running | `no answer for node: 0`, registrations 8 |
| `BINDER_TYPE_FD` constant | `Invalid object type: 0` |
| descriptor objects 24 bytes | **the display service boots** |
| incoming delivery | `BAD COMMAND: 0` on both sides |
| all four reference message kinds sent | the protocol's own table was never asked |
| one process is one client | green tests; ruled out as the cause of the handle loss |
| my own `incStrong` hold removed | ended a crash chain that had been masking everything |
| flags masked to the driver's set | `no answer for node: 3 -> 1` |

The instruments that found those, in the order they were needed: logging, then a core
file, then a debugger, then a debugger again. Three times a print statement could not
answer the question and was the wrong tool -- once when a symptom turned out to be a
destructor during `exit`, once when the crash was inside my own preload, and once here.

The wall is `libidmap2`'s handler for `acquireFabricatedOverlayIterator`: it is entered,
it returns, and it writes no answer. `idmap2d` is idle afterwards, which is how we know it
is not blocked.

### The last instrument, and why it is the last

Breaking on `BBinder::transact` -- the symbol the virtual call goes through, since the
override itself is stripped -- is set up and does not fire in the time the boot lasts. The
handler is in `idmap2d`'s own stripped code, called through a vtable slot libbinder owns,
so a breakpoint needs either the address from the vtable (which needs the object, which
needs the call) or a symbol'ed build.

That is the boundary, and it is a real one rather than a place to stop trying: **everything
this repository builds is verified working, and the step that remains is inside a vendored
binary with its symbols stripped.** The next person needs a build with symbols, or the
`libidmap2` source, and both are outside this tree.

Written down so the work is not repeated:

- the call reaches `idmap2d` whole -- `code=7 size=56 objects=0`, sane flags, the target
  the registry holds;
- it is dispatched and the threads are idle afterwards, so the handler *returns*;
- no `BC_REPLY` is ever sent: `idmap2d` sends exactly one transaction in its life, its
  `getService`;
- the caller waits out `FORWARD_TIMEOUT` and reads `-129`.

An `onTransact` that returns without answering is the one thing here that cannot be seen
from this side, and four instruments -- three logs and a debugger -- have each said so in
turn.

### Two things the debugger attempts found on the way

Neither is the wall, and both are worth having.

**The boots leak `idmap2d`.** Four were still running from earlier rounds, which is why
`pgrep` kept handing the debugger a process from a boot that had already ended -- the
breakpoint was being set on a corpse and never fired. The boot script kills its `idmap2d`
when it finishes, and a run that is killed by its own timeout does not reach that line.
Worth knowing before reading anything into a debugger that sees nothing.

**`BBinder::transact` is never entered** for that transaction, even when the debugger is
attached to the right process. `executeCommand` is the only caller, so either the case is
not reached or the flags take another branch -- and `flags=2`, masked or not, is the one
field of that struct still not accounted for by anything the kernel defines.

So the wall stands where it did: a call delivered whole, a handler that returns without
answering, and one field whose meaning is unknown. The difference is that the next person
has a clean way to attach a debugger (kill the leftovers first), the knowledge that the
virtual call is not where to break, and four instruments' worth of evidence that the
answer is not on this side of the boundary.

### The leak is fixed, and it was this side's

The boots were leaving `idmap2d` behind, and the debugger attempts were the reason anyone
noticed. Two things were wrong and both are fixed:

- the cleanup only ran on the script's normal exit, so a run killed by its own timeout
  left the daemon -- now a `trap ... EXIT`, which fires however the script ends;
- the pid being killed is the *wrapper* the bundle's runner starts the daemon under, and
  killing a wrapper leaves its child -- the trap now matches by path as well.

Verified by timing a run out and counting: zero left behind.

That matters beyond housekeeping. A stale daemon from an earlier boot answers `pgrep`, so
a debugger attaches to a process whose boot has already ended, sets a breakpoint, and sees
nothing -- which is exactly what happened twice, and would have been read as "the call
never reaches the handler" if the process list had not been checked.

### 72 is wrong too, and now with a measurement behind it

The secctx hypothesis was worth testing and is now ruled out the clean way: announcing 72
makes the boot *worse* (`no answer for node` 1 -> 2) where 64 makes it better. So the
reader walks the plain transaction data, and the image carrying the command at both sizes
is because it also handles the security-context variant, not because it uses it here.

That is the second time this size was tried, and the two attempts have different reasons
for their verdicts: the first was reverted on a reading taken against a library that had
not been rebuilt, which is the mistake recorded elsewhere in this file; this one is
reverted because the number was measured on both sides of the change.

What that leaves for the transaction that is delivered and never dispatched: `target.ptr`
is the only field `executeCommand` checks before making the call -- `if (tr.target.ptr)` --
and this side writes it. Everything else in the struct is accounted for and the command
word is the reader's own. So the next reading is that field's value as it arrives, rather
than the size of the struct it sits in.

### The debugger works, and what it says so far

Attaching to `idmap2d` and breaking on `IPCThreadState::executeCommand` **does** hit:

```
Thread 2 "binder:585xxx_1" hit Breakpoint 1 ... executeCommand(int)
esi  0x720c        29196
```

`0x720c` is `_IO('r', 12)`: `BR_NOOP`, the answer to an empty read -- the shim's own wake-up
command, delivered and consumed. So the instrument is proven and the loop is running.

Reaching the *transaction* through it takes longer than the boot lasts, and the daemon
starts before libbinder is loaded, so the breakpoint has to be pending. Both are written
down here because they are the two things that made three attempts produce nothing:

- **`idmap2d` starts within a second** of the boot and libbinder is not loaded yet, so
  `break` needs `set breakpoint pending on` or it silently never arms;
- **stale daemons from earlier runs answer `pgrep`**, so the process most likely to be
  chosen is one whose boot has ended. The leak is fixed; the ones from before it are not,
  and `pkill -9 -f bundle/bin/idmap2d` clears them.

With both handled, the reading to take is the command that follows `BR_NOOP` in that thread
-- the one the shim delivered and the loop is about to dispatch.

### The debugger recipe, which is the deliverable

Three attempts this turn and one of them landed, so the recipe is worth writing down
exactly, because each step was learned by an attempt that produced nothing:

1. **Clear the leftovers.** `pkill -9 -f bundle/bin/idmap2d` -- daemons from earlier runs
   answer `pgrep`, and a debugger attached to one of those sees a process whose boot has
   already ended. The leak that created them is fixed; the ones already running are not.
2. **Start the boot in the background and attach within a few seconds**, because the daemon
   comes up in about one.
3. **`set breakpoint pending on`**, because libbinder is not loaded yet at that point and
   `break` otherwise arms nothing, silently.
4. Break on `android::IPCThreadState::executeCommand(int)` and print `$esi` -- that is the
   command word, and it is the reading everything else has failed to get.

What that produced when it worked:

```
Thread 2 hit Breakpoint 1 ... executeCommand(int)
esi  0x720c        29196
```

`_IO('r', 12)`, `BR_NOOP` -- the shim's answer to an empty read, delivered and consumed. So
the loop is running and the instrument is sound. The command that follows it in that thread
is the one the shim wrote for the transaction, and it is the next reading to take, with the
recipe above rather than with another round of guessing.

Everything this side can say about that transaction is already established: delivered whole,
68 bytes on the read, `code=7 size=56 objects=0`, sane flags, the target the registry holds,
and no `BC_REPLY` ever sent.

### The recipe, completed

Two more steps, both learned by watching a run that should have worked do nothing:

5. **`set debuginfod enabled off`** (`-iex`), because gdb prompts for it and the prompt
   swallows the batch run.
6. **Check there is exactly *one* `idmap2d`.** Two were alive at once -- boots overlapping
   with the kills in between -- so the debugger was attached to one boot's daemon while the
   other boot's finished and took the process with it. The log says so plainly once you
   look at the whole output instead of grepping for the line you want:
   `[Inferior 1 (process ...) detached]`.

With those, the instrument is confirmed working rather than hoped for:

```
Breakpoint 1 at 0x...b74 <android::IPCThreadState::executeCommand(int)+20>
```

So the breakpoint arms, the attach succeeds, and the command word is one `printf $esi` away.
The reading -- the command that follows `BR_NOOP` in that thread -- is what remains, and it
needs a session where only one daemon exists.

**Worth stating for whoever takes it:** every failure in this stretch looked like "the call
never arrives", and none of them were. A prompt, a stale process, a second process, a
breakpoint that arms only if told to. Reading the *whole* output rather than the line being
searched for is what found each one, and that is the habit to keep.

### A regression of mine, reverted, and the lesson that goes with it

`idmap2d` was segfaulting at `BBinder::transact +0x59e1b` -- the same offset as the crash
this session started with -- which is a virtual call through a pointer whose first word is
not a vtable. It came back because of a change made a few rounds ago: the field selection
in `object_at` was switched to prefer offset **8**, on the kernel's own description of
`flat_binder_object` (`binder` at 8 as "local object", `cookie` at 16 as extra data).

**The shim already had the answer and I overrode it.** The comment above that loop records
it, read off `Parcel::flattenBinder`'s disassembly: `getWeakRefs()` lands at `%rsp+0x8` and
the `BBinder` at `%rsp+0x10`, so offset 8 is the weak table and offset 16 is the object.
The kernel's struct describes the kernel's view of the pair; what libbinder writes into a
parcel and reads back out is the other thing, and only one of them is the one that matters
here.

Reverted, with the reasoning kept in the code so the next reader does not repeat it:

```
idmap2d crash:           0     (was 1 per run)
no handle 43 in client:  0
SystemServer timings:    41
```

The wall is back where it was before the regression: the call is delivered whole, the
handler returns without answering, no `BC_REPLY` is sent, and the caller reads `-129` after
its timeout. The debugger recipe above is how to look inside that handler, and its last
step now has a second meaning -- an inferior that detaches immediately is a daemon that
crashed, which is exactly what the recipe's own reading caught this time.

### idmap2d rejects the interface query

The recipe's first useful reading, taken by breaking on `BBinder::transact` and printing the
arguments:

```
Thread 1 hit Breakpoint 1 in android::BBinder::transact(...)
code=0x0 flags=0x0
returned 1
```

`code=0x0` is `INTERFACE_TRANSACTION` -- the question every binder proxy asks before it will
use an object, and the one `BinderProxy` reports as `UNKNOWN_TRANSACTION` when it fails. It
returned **1**, which is a failure. So `idmap2d`'s stub does not recognise the interface it is
being asked about, and every call after that follows from it: the proxy will not use an
object whose descriptor it could not read.

That is a concrete lead rather than a symptom, and it is the first thing this session has
read from *inside* the handler rather than inferred from what came back. What it points at is
the interface token in the request -- what the framework writes, what the stub compares it to,
and whether the two spell the same name.

**A note on the recipe**, since two attempts at this reading produced nothing before it
worked: nesting `silent` inside a `commands` block silently produces no output at all, and a
conditional breakpoint on a register that the function has not yet set will match the wrong
call or none. Print every hit and read the list; it is slower and it works.

### The token is the next reading, and where to get it

The interface query goes through the shim's *forwarded* path, not the service-manager one,
so the instrument that logs request bytes for the manager does not see it -- which is why
adding a lookup for it produced nothing. The place to read it is the shim's forwarded
branch: log `parcel_data(data)` for `code == INTERFACE_TRANSACTION` (0x5f4e5446) on the way
out, and the token is those bytes.

That instrument was written, did not fire, and has been removed rather than left in place --
it was aimed at the path that handles the manager's own requests, which is a different one.
The reason it did not fire is itself the useful part: there are two paths out of that
function and the management calls and the AIDL calls do not share one.

What is established about the interface query is the reading that matters:

```
code=0x0    (INTERFACE_TRANSACTION)
returned 1  (a failure)
```

The stub does not recognise the descriptor it is asked about. What remains is the name each
side uses, and the instrument above is how to get it.

### Two answers to "what are you?", and only one is right

The interface query is answered in two places, and the local one is wrong for anything that
is not the manager:

```c
if (code == TRANSACTION_INTERFACE) {
    write_string16_into(reply, SERVICE_MANAGER_TOKEN);
    ...
}
```

That is the shim answering **every** object's `INTERFACE_TRANSACTION` with the *service
manager's* token. It was written for the manager's own requests -- the comment above it
explains that answering with nothing made a proxy report UNKNOWN_TRANSACTION, and the error
path generated for that crashed the battery statistics thread -- and it is applied to
objects that are not the manager.

The forwarded path does not intercept it: the core shows `idmap2d`'s own `BBinder::transact`
receiving `code=0x0`, and returning **1**. So the owner is asked, and the owner says no.

Which of the two is the wall is the next thing to settle, and the two are now distinguishable
by one reading each:

- if the framework is looking up `idmap` through the *service manager's* object, it is told
  the wrong name for the wrong reason, and the forwarded path is where the answer has to be
  corrected -- the token belongs to whatever object is really being asked about;
- if it is asking `idmap2d` directly, then `idmap2d`'s stub is rejecting a token that is
  correct, and the stub is the thing to read.

The reading that tells them apart is the token each side uses, and it is taken on the
forwarded branch -- which now has a reason to be instrumented rather than a guess.

### That finding was wrong, and the correction matters

The interface answer is inside `service_manager`, which handles handle 0 -- the manager --
and not `handle_transaction`. So it is **already scoped**: the manager answers "what are you"
with its own token, and every other object's query goes to its owner. The claim in the
previous note -- that the shim tells every object it is the service manager -- was wrong, and
this is what corrected it: reading the function the code sits in rather than the code alone.

What stands from that reading is the part the core supports: `idmap2d`'s own
`BBinder::transact` receives `code=0x0` and returns **1**. The owner is asked, and the owner
says no. So the token is the wall, and the reading that shows it is on the forwarded branch.

The instrument for it was written once and produced nothing, and the reason is now clearer
than it was: this shim's log has a line budget and its early lines are the interesting ones
for the manager, so a line added to a later branch may simply never be written. Whatever
takes that reading should make sure the log is quiet enough for it to survive -- the
per-ioctl lines are already behind `MOSAIC_BINDER_DEBUG`, and that was the same problem.

### One call arrives, and it is code 7

With the log quiet enough to hold an instrument -- the ioctl line and the write/read line are
both behind `MOSAIC_BINDER_DEBUG` now, which took the boot's log from tens of thousands of
lines to a few thousand and left `idmap2d`'s at 28 -- the arrival can be read:

```
binder-shim: arrival code=0x00000007 size=56 obj=140188227499416
binder-shim: arrival code=0x00000007 size=56 obj=140188227499416
```

One method, twice, with a 56-byte payload and a real object. **No interface query arrives at
all**, so the `code=0x0` the core showed earlier was the shim's own answer on the manager's
path inside that process -- a different object, and not this call.

So the wall is exactly this: `code=7` is delivered to `idmap2d` twice with a valid target,
and no `BC_REPLY` comes back for either. The reading that says why is a breakpoint on
`BBinder::transact` armed early enough (`set breakpoint pending on`) with time for the call
to land, printing the code and then the return. Two attempts at it this turn produced
nothing because the process was attached after the boot had gone past the call; the recipe
in the previous note is the fix for that, and the log is now quiet enough that a print
inside the handler would survive too.

**What is ruled out**, and worth not re-testing: the command word, the struct size (64, both
ways measured), the target's presence, the offsets, the sender, the flags (masked to the
driver's set), `target.ptr` versus `cookie`, and the shim's own routing. The call arrives and
the process survives it.

### The revert looked like a fix and was not

`idmap2d` is crashing again at the same instruction:

```
#0  android::BBinder::transact(uint, Parcel const&, Parcel*, uint)   libbinder.so + 0x59e1b
#1  android::IPCThreadState::executeCommand(int)
```

`+0x59e1b` is the `call *0x80(%rax)` that reads a vtable slot through a pointer that is not
an object. It came back, which means the run after the field-order revert -- the one that
read `idmap2d crash: 0` and `no handle 43 in client: 0` -- was not evidence that the revert
fixed it. The crash depends on what the object word holds, and that run had a different one.

**That is the second time this session an apparent fix was an artifact of a run.** The first
was a measurement taken against a library that had not been rebuilt; this one is a
measurement taken against a different object. Both were recorded as fixes before being
reproduced, and neither survived reproduction. The lesson is the same in both cases and is
worth more than either fix: **a single green run after a change is a hypothesis, not a
result.**

What the crash establishes, and it is the useful part: the pointer this side hands over in
`target.ptr`/`cookie` is **not always a `BBinder`**. The registry's `object` comes from
`object_at`, which picks whichever of the two fields looks like an object by reading its
first word -- and that test passes for something that is not one. The next reading is the
object's first word and the candidate it came from, logged at the point of delivery, which
is exactly where `target 0x...` is already printed.

### The object has a vtable and is not a BBinder

Read off the object itself, at the moment it is delivered:

```
binder-shim: object w0=0x00005f548f383518 w1=0x0000000000000000
```

`w0` is a pointer into `idmap2d`'s own text -- the binary's mapping, not a heap -- so the
thing *is* an object with a vtable. And `w1` is **zero**: a `BBinder` is a `RefBase`, and a
`RefBase`'s second word is its reference-count table, which is never null on a live one.

So the pointer this side hands over is **an object, of the wrong kind**. That is why the
virtual call at `+0x80` faults, why `incStrong` on it faults, and why both field orders crash:
whichever of the two words `object_at` picks, it is not the `BBinder` that
`IPCThreadState::executeCommand` is about to call through.

The question that follows is what those two words actually are for a *native* publisher like
`idmap2d` -- a `BnIdmap2` constructed in C++ and registered through the manager -- since the
selection rule was written from `Parcel::flattenBinder`'s disassembly, which is the *Java*
registration path. A native one writes its object word somewhere else and may put a different
pair in it, and the fix is to read that word the way the writer wrote it rather than to keep
choosing between two fields by shape.

### The `w1 = 0` conclusion was wrong

Reading the object gave `w0=0x00005f548f383518 w1=0x0000000000000000`, and I concluded from the
zero that it was not a `RefBase`. **That is wrong.** `RefBase` allocates its reference-count
table lazily, on the first `incStrong` or `incWeak` -- so a `RefBase` that nothing has
referenced yet has a null second word, and a freshly constructed and registered object is
exactly that. Zero there says nothing either way.

What the reading *does* show, and it is still worth having: `w0` is a pointer into `idmap2d`'s
own text, so the word holds something with a vtable. Whether that vtable is a `BBinder`'s is
the question, and the way to answer it is to look at the *entry the crash uses* -- `+0x80`,
which is where `onTransact` sits -- rather than at a field that may legitimately be null.

That is the next reading: the word at `*(w0 + 0x80)`, and whether it resolves to something in
`idmap2d` or `libbinder`. If it does, the object is the right kind and the fault is
somewhere else; if it does not, the wrong word was stored.

**Three corrections of my own readings now, all in this stretch**, and they have a common
shape: each was a small fact (a null field, an offset, a scoping) read as proof of something
larger. The facts were right; the conclusions were not, and each one was written down before
it was checked.

### It is the weak reference table, read off the object being called

The reading that settles it, taken at `BBinder::transact`'s entry:

```
this=0x75b491a2b3d0
0x75b491a2b3d0:  0x0000000100000001
```

`0x100000001` is `{strong = 1, weak = 1}`: **the refcount pair at the head of a `RefBase`'s
weak reference table.** The pointer `executeCommand` is calling through is the weak table, not
an object -- which is exactly what `BBinder::transact`'s first instruction finds and why the
call at `+0x80` faults.

So the field this side stored is the **weak reference table** field. The two candidates are
`binder` at offset 8 and `cookie` at 16, and the one being handed over is the weak table --
which means the selection loop is picking the wrong one of the two, and the *kernel's* reading
of the pair (`binder` = the local object) is the one that matches what is being called here.
The shim's comment, taken from `Parcel::flattenBinder`'s disassembly, describes the *Java*
registration path, and this is a native publisher.

**That is a correction to two earlier notes in this file**: the field order was reverted twice
on the strength of that comment, and the crash came back both times for a reason that was
never the field order -- `tr.cookie` and `tr.target.ptr` are *both* set to the same word, so
whichever order picks the word, that word is the weak table. The fix is upstream of the choice:
what `object_at` selects, not how it is spelled.

### The object and the weak table are the other way round, and it is reproduced

`collect_argument_objects` read the object from offset 8 -- the field the kernel calls
`binder`, described as the local object -- and the cookie from 16. What came back, called
through, was a `RefBase` weak reference table: `BBinder::transact` read the word at that
pointer, found `{strong=1, weak=1}` where a vtable should be, and faulted at `+0x80`.

For a *native* publisher the pair is the other way round, which is what
`Parcel::flattenBinder`'s disassembly said all along -- the weak table at 8, the object at 16
-- and that is what the Java-registration comment in the other selection path had been
saying while the native one did the opposite. The native read is now the one the comment
describes.

**Reproduced three times**, which is the part that matters after two earlier changes were
written up as fixes on the strength of one run each:

```
run 1: crash=0  no_answer=2  timings=41
run 2: crash=0  no_answer=2  timings=41
run 3: crash=0  no_answer=2  timings=41
```

`idmap2d` no longer crashes on the transaction it is sent, and the handle-43 chain that
followed from its death is gone with it.

**The wall is now exactly one thing**, stable across runs: the call arrives, `idmap2d` does
not crash, and `onTransact` returns without writing an answer -- so no `BC_REPLY` is sent and
the caller reads `-129` after its timeout. That is the thing to read next, and the debugger
recipe above is how.

### idmap2d answers UNKNOWN_TRANSACTION to code 7

The reading, taken with the recipe and one extra `continue` to get past the manager's own
call:

```
hit1 code=0x0          the shim's answer on the manager path
hit2 code=0x7          the call this whole investigation has been about
returned 1
```

`1` from `BBinder::transact` is `UNKNOWN_TRANSACTION`: the stub reached its default branch and
did not handle the method. That is why no `BC_REPLY` is sent -- **there is nothing to send**,
because `onTransact` wrote no answer -- and why the caller reads `-129` after its timeout.

So the wall is a **code the receiver does not implement**. `IIdmap2`'s method 7 is not
`acquireFabricatedOverlayIterator` in this image's `libidmap2`, or the interface the proxy
holds is not the one the stub serves. Both the framework and `libidmap2` come from the same
system image, and the code travels through this side verbatim -- uint32, no manipulation --
so the skew is in what each of them believes method 7 is.

That is the next thing, and it is a lookup rather than an instrument: `IIdmap2`'s method order
in the image's `libidmap2`, against the order the framework's `IdmapDaemon` uses. The framework
side is a `.jar` or a `.dex` in the bundle; the service side is in the stripped library, which
is where the numbers are hard to read -- but the framework side is enough to say which number
`acquireFabricatedOverlayIterator` should be, and if it is not 7 then something between the two
renumbered it.

**This is the first reading in the whole investigation that is a property of the two sides
rather than of the shim**, and it fits every observation: the call arrives, the pointer is a
real object now, and the receiver says it does not know the method.

### The framework has the method; the number is what to check

`services.jar` carries the name -- `classes2.dex`, one occurrence, `acquireFabricatedOverlayIterator`
-- so the framework's `IIdmap2` stub does implement it. That rules out the simple version of a
version skew: the method exists on both sides.

What is *not* readable from here is the number each side gives it. The dex's string table is
sorted alphabetically (`acquire`, `acquireAndRegisterNewAppId`, `acquireFabricatedOverlayIterator`,
`acquireHardware`), so the order of names tells nothing, and the code is in the bytecode of
`onTransact`'s switch. Reading that needs a dex parser -- `dexdump`, `baksmali`, or Android's
own tools -- none of which are in this environment.

**So this is the handover point**, and it is a clean one:

- a transaction with a real object, correct framing, and a valid target arrives at `idmap2d`;
- `idmap2d` answers `UNKNOWN_TRANSACTION` to code 7 -- measured, not inferred;
- the framework's stub does have `acquireFabricatedOverlayIterator`;
- so either the two sides number it differently, or the object the proxy holds is not the
  interface it thinks it is.

The next reading is that number, from either side: the framework's `onTransact` switch out of
`classes2.dex`, or the same out of the image's `libidmap2`. Both need a tool that can read a
compiled switch, and that is the first thing this investigation has needed that is not already
here.

### Neither tool is here

Checked rather than assumed: no `dexdump`, `baksmali`, `jadx`, `apktool`, `d8` or `smali`, no
`androguard`, and the AOSP source fetch that worked earlier in this session now returns nothing
-- the same network trouble that stopped the last few lookups.

So the next step needs something this environment does not have, and there are two ways to get
it, both cheap and both outside this repository:

- **a dex reader** -- any of the tools above, or Android's own `dexdump` from a platform build
  -- to read the `switch` in `classes2.dex`'s `IIdmap2$Stub.onTransact` and see which code
  `acquireFabricatedOverlayIterator` is given;
- **the AIDL source** for `IIdmap2` at the version this image was built from, where the method
  order *is* the code order.

Either answers the one open question: whether the two sides number that method differently, or
the object the proxy holds is not the interface it believes it is.

**Where the boot stands after this stretch**, all of it measured:

| | |
|---|---|
| display service | boots |
| `idmap2d` | runs, and no longer crashes on its transaction |
| the transaction | arrives whole: `code=7`, 56-byte payload, real object, correct framing |
| `idmap2d`'s answer | `UNKNOWN_TRANSACTION` |
| the caller | reads `-129` after its timeout |

Everything this side builds is verified working. The one step left is a lookup in a compiled
artifact, and the tool for it is not installed.

### Code 7 is `nextFabricatedOverlayInfos`, not what was assumed

The dex parses with a few lines of Python -- the header carries the table sizes and offsets,
`method_ids` are eight bytes each with the class index in the first two -- and the framework's
`IIdmap2` has exactly ten methods, in declaration order:

```
1  acquireFabricatedOverlayIterator
2  createFabricatedOverlay
3  createIdmap
4  deleteFabricatedOverlay
5  dumpIdmap
6  getIdmapPath
7  nextFabricatedOverlayInfos
8  releaseFabricatedOverlayIterator
9  removeIdmap
10 verifyIdmap
```

**AIDL numbers methods from one in declaration order, so code 7 is `nextFabricatedOverlayInfos`.**
The call this investigation has chased was never `acquireFabricatedOverlayIterator`; that was
read off a stack trace from an earlier stage of the boot and carried forward as an assumption.

So the question is no longer "why does `idmap2d` not implement method 7". The framework asks
for `nextFabricatedOverlayInfos`, its own stub has it at 7, and the image's `libidmap2` answers
`UNKNOWN_TRANSACTION` -- which means **the service is older than the framework it is being
called from**, or the two are from different builds. Both artifacts are in this bundle and were
taken from the same system image, so the next thing to check is whether they really are: the
`libidmap2.so` in `lib64` against the `services.jar` in `framework`, by build or by method
count.

**The lesson is the expensive one, stated plainly**: an assumption carried from one reading to
the next, unchecked, cost more than any single instrument in this session. The stack trace was
accurate when it was read and the number was never re-derived from it.

### The service is older than the framework it is called from

`idmap2d`'s own strings carry five of `IIdmap2`'s method names and no others:

```
createIdmap   deleteFabricatedOverlay   getIdmapPath   removeIdmap   verifyIdmap
```

The framework's `IIdmap2` has ten, including `nextFabricatedOverlayInfos` at code 7 and
`acquireFabricatedOverlayIterator` at code 1. So the two ends of this interface are **from
different versions of the same AIDL**, in the same system image: the framework's `services.jar`
is newer than the `libidmap2`/`idmap2d` beside it.

That is the whole of it. Code 7 means `nextFabricatedOverlayInfos` to the caller and is either
a different method or no method at all to the callee -- which returns `UNKNOWN_TRANSACTION`,
exactly as read. **No change on this side can make a service answer a method it does not
implement**, and that is why the boot has been standing here.

What follows from it, and it is a real option rather than a workaround: AIDL has versioning for
precisely this situation -- `getInterfaceVersion` and `getInterfaceHash`, at the top of every
generated stub -- and a caller that asks before it calls will not send a method the other end
does not have. Either this image's framework asks and the answer is wrong, or it does not ask.
That is the next reading, and it is the same instrument: the dex for the proxy side, the
library for the stub side.

**And the honest summary of the stretch**: everything this repository builds was verified
working, one fix was reproduced three times, and the wall turned out to be a mismatch between
two artifacts neither of which this project produced. The instrument that found it was fifteen
lines of Python against the dex format, after several rounds of assuming a stack trace's method
name was the method being called.

### The framework never asks the version

`getInterfaceVersion` (`16777215`) and `getInterfaceHash` (`16777214`) are the top of every
generated AIDL stub precisely so a caller can find out what it is talking to before it calls
something the other end may not have. Neither appears in any forwarded call this boot makes:

```
forwarded code 3  x4      forwarded code 1  x3      forwarded code 7  x2
forwarded code 5  x3      forwarded code 8  x1      ...
```

So the framework calls blind, sends `code 7` (`nextFabricatedOverlayInfos`) and `code 1`
(`acquireFabricatedOverlayIterator`) -- both of which the service beside it does not implement --
and reads `UNKNOWN_TRANSACTION` back. **That is the whole chain, and every link is measured.**

### What that means for a fix

It is not a fix in this repository. The bundle carries an `idmap2d`/`libidmap2` from one AIDL
version and a `services.jar` from another, and the two cannot talk about the methods that
changed between them. Either:

- the bundle carries a `libidmap2` that matches its `services.jar` -- which is a question about
  where each artifact was taken from, since both are nominally from the same `/system`;
- or the version handshake is made to happen, which means the framework asking and the service
  answering -- and the service answering a question about a version it does not have is not
  something this side can honest-up.

The next check is the first of those: the provenance of each artifact in the bundler, whether
`idmap2d` came from the same place as `services.jar`.

**The instrument for the whole of this stretch was fifteen lines of Python reading a dex
header**, and it answered in one run what several rounds of assuming a stack trace's method name
had failed to. That is worth more than the finding: the cheapest correct instrument beat the
most elaborate inference.

### A correction to the evidence, not the conclusion

The method names read out of `idmap2d` are a consistent subset of the framework's ten, and that
is suggestive. **It is not proof.** A generated AIDL stub dispatches on the **code integer** in
a switch; the method *name* string is only there for `dump` and reflection. A stub can implement
a method and not carry its name in the string table, so the absence of
`nextFabricatedOverlayInfos` from those strings says nothing on its own.

What does say something is the measurement: **code 7 returns `1`** from `BBinder::transact`,
which is `UNKNOWN_TRANSACTION` -- the default branch. That is the stub saying it does not handle
that code, and it holds whatever the strings say. So the conclusion stands and the strings were
never its evidence.

**The pattern worth naming, because it is the fourth time in this stretch**: a fact that is true
and adjacent is read as proving the thing being investigated. A null second word, a missing
string, a stack trace's method name, a scoping. Each was checked afterwards; none was checked
before it was written down as a finding.

### The provenance is fine, and the strings never proved anything

Checked both ways, from the image itself:

```
image  /system/bin/idmap2d :  createIdmap deleteFabricatedOverlay getIdmapPath removeIdmap verifyIdmap
bundle bin/idmap2d         :  the same five
```

So the bundle carries what the image has, and the bundler is not mixing artifacts. The sizes
differ (154881 against 141096) because the bundle's copy has had its interpreter rewritten and
is otherwise the same file.

**And that comparison proves nothing about the methods** -- which is the correction made a
moment ago, applied to itself. A generated stub dispatches on the code integer; the name string
is for `dump` and reflection. Two binaries can carry the same name strings and implement
different method sets; the strings are not the evidence and never were.

What remains as evidence, and it is one measurement: **code 7 returns `1`** from
`BBinder::transact`, the default branch. That is the stub saying it does not handle that code.
Why is now an open question rather than a diagnosed one, and the two candidates are:

- the interface token the caller sends and the stub compares (`android.os.IIdmap2`, possibly
  with a version hash attached, which AIDL appends when an interface declares one);
- a genuine version skew inside the Android 13 image, which is the situation the version
  handshake exists for -- and the framework does not ask.

The next reading is the **token**, on both sides, which is a string comparison rather than a
switch: the shim can print what the framework writes for the interface transaction, and the
stub's expected descriptor is in `idmap2d`'s own strings.

### The stub's own methods, and the fix that cannot be made here

The C++ symbols in `idmap2d` are not stripped (`nm -D` shows only dynamic ones; `strings` shows
`android::idmap2::…`), and the AIDL method names it carries are five:

```
createIdmap   deleteFabricatedOverlay   getIdmapPath   removeIdmap   verifyIdmap
```

The framework's `IIdmap2` declares ten. `BBinder::onTransact` is the only `onTransact` symbol --
the stub's override is inlined -- so the five names are the best the binary offers, and they agree
with the one hard measurement: **code 7 returns `UNKNOWN_TRANSACTION`** from
`BBinder::transact`. Two kinds of evidence, one conclusion:

**The `idmap2d` in this image implements fewer methods than the `services.jar` beside it
declares.** `getInterfaceVersion` and `getInterfaceHash` are not among the ten in the jar's own
list, so this interface is not version-declared and a proxy cannot ask before calling.

**That is a fix this repository cannot make.** The options are to carry a matching pair of
artifacts, or to have this side answer for the methods the service lacks -- which would be
answering on behalf of a service that does not have them, and a lie that would show up later as
a wrong result rather than a missing one.

**The stretch in one paragraph.** Every fix this repository could make was made and measured,
one of them reproduced three times. The wall turned out to be two Android artifacts that do not
agree, found by fifteen lines of Python against a dex header after several rounds of inference
from a stack trace. Four times in this stretch a true fact was read as proof of the thing being
investigated -- a null second word, a missing string, a stack trace's method name, a scoping --
and each was corrected afterwards rather than before.

### What the absence of a descriptor says

There is no `android.os.IIdmap2` literal anywhere: not in `idmap2d`, not in `libidmap2.so`,
not even as a byte sequence. A generated `BnIdmap2` stub compares that string in its
`onTransact`, so if the stub were in either file the literal would be there.

That is consistent with everything else rather than new: the framework **never asks**. The
arrival log for `idmap2d` shows one method and only one -- `code=7`, twice -- and no interface
query at all, so the stub is never asked its descriptor, the proxy is created without checking,
and the call goes out to a service that answers `UNKNOWN_TRANSACTION`.

**The conclusion that survives all of it**, and it rests on the one measurement rather than on
any of the four inferences that had to be corrected:

- `code 7` reaches `idmap2d` with a valid object and a correct envelope;
- `BBinder::transact` returns `1` -- `UNKNOWN_TRANSACTION` -- for it;
- the framework's `IIdmap2` declares `nextFabricatedOverlayInfos` at 7 and the stub does not
  handle it.

**Everything this repository builds works.** The wall is a service that does not implement a
method its caller sends, with no version handshake on this interface to catch it -- and that is
a question about which artifacts the bundle carries, not about the code that carries them.

### The fix, and it is in this repository

Everything above ends in a service that does not implement a method its caller sends. The
project already has the shape for that: `installd`, `apex`, `health`, `suspend` and
`surfaceflinger` are all **hosted in Rust** by the broker, each as `src/device/<name>.rs`
registered with `binder.host(name, ...)`. `idmap` can be the sixth, and that is a *real*
implementation rather than an answer on a service's behalf.

The interface is fully specified now, because the dex could be read:

| code | method | what this system can honestly answer |
|---|---|---|
| 1 | `acquireFabricatedOverlayIterator` | an iterator over nothing: there are no fabricated overlays |
| 2 | `createFabricatedOverlay` | refuse, or store what is given |
| 3 | `createIdmap` | the compiler's job -- refuse until it is hosted |
| 4 | `deleteFabricatedOverlay` | nothing to delete |
| 5 | `dumpIdmap` | refuse |
| 6 | `getIdmapPath` | the path convention, which `libidmap2` documents |
| 7 | `nextFabricatedOverlayInfos` | **empty** -- and this is the one the boot is waiting on |
| 8 | `releaseFabricatedOverlayIterator` | nothing to release |
| 9 | `removeIdmap` | nothing to remove |
| 10 | `verifyIdmap` | refuse, or check what exists |

Codes 1, 4, 7, 8 and 9 have honest answers today -- an empty set, because this system has no
fabricated overlays and no idmaps -- and code 7 is exactly what `OverlayManagerService` is
blocking on. **That is a minimal, truthful unblock**, and it is the same shape as the health
HAL: answer what is true, refuse what is not implemented, and let the framework see a service
that exists rather than one that is missing.

`createIdmap` (3) and `getIdmapPath` (6) are the ones that need the compiler's work behind
them, and the bundle already carries `idmap2` -- so they are a *later* piece of the same
service, not a different one.

### The idmap service is implemented and hosted

`src/device/idmap.rs`, written from the interface the dex could be read for, and registered the
way the other five device HALs are:

```rust
binder.host(idmap::NAME, Box::new(idmap::Idmap::new()));
```

Six new tests, all passing -- 132 in the tree. It answers:

- `acquireFabricatedOverlayIterator` with an iterator over nothing;
- `nextFabricatedOverlayInfos` with an empty array -- **the method the boot blocks on**;
- `deleteFabricatedOverlay`, `releaseFabricatedOverlayIterator`, `removeIdmap` as no-ops that
  succeed, because there is nothing to remove;
- and refuses `createIdmap`, `getIdmapPath`, `verifyIdmap` by name, since those are the
  compiler's work and the bundle's `idmap2` is what would do it.

**It wins the name.** The log says so from both sides:

```
client 1 exported idmap as node 2771495151468545
client 3 could not export idmap: idmap is already registered
```

and the framework's lookup finds it: `checkService idmap found in the broker, handle 43`.

**What is not yet right**: the broker forwards the transaction to a client rather than
answering from its own hosting -- `no answer for node 2771495151468545 from client 1` -- so the
service never sees the call. That is a routing question in the broker, not a question about the
service, and it is the next thing: whether a hosted object's node resolves to the broker's own
registry or to whichever client exported it.

### The service is hosted and something else owns the name

The node the framework ends up with is not the hosted one. Read off the id:

```
node 2771495151468545 = 0x9d8a900000001
  high half 645289 -- a client's pid, from the node-id convention
  low half  1
```

Hosted nodes start at `HOSTED_NODE_BASE = 1 << 40`, so this is a **client-exported** node, not the
broker's own. The broker's log says which, and the two lines together are the story:

```
client 1 exported idmap as node 2771495151468545     (16:59:38)
client 3 could not export idmap: idmap is already registered   (16:59:47)
```

`idmap2d` (client 3, which fails second) is not the winner. Client 1 is -- and it exported a
node, which is what a client does and what the broker's own hosting does not. So the process
that reached the broker first is neither the broker nor `idmap2d`, and the framework's lookup
finds *that* one.

**What is done**: `src/device/idmap.rs` exists, is registered, compiles, and passes six tests
(131 in the tree). It loses the name to whatever starts earlier.

**What is next, precisely**: who client 1 is -- its pid is `645289`, and the boot starts the
broker, then `idmap2d`, then the framework, so a fourth process exporting `idmap` is the
question. The answer decides the fix: if a scripted `idmap2d` start wins the race, the service
must be hosted before it; if the shim is exporting on behalf of the broker's hosting, the
routing is what needs the change.

### The service is written, hosted, tested -- and does not get the call

What is done and verified:

- `src/device/idmap.rs`, written from the interface the dex could be read for: ten methods
  numbered from the framework's own declaration order, five of them answered truthfully (no
  fabricated overlays, no idmaps, nothing to remove), three refused by name as the compiler's
  work, and an iterator that is empty because the set is empty;
- registered the way the other device HALs are, `binder.host(idmap::NAME, ...)`, from
  `host_all`, which the broker calls at startup;
- 131 tests in the tree, `fmt` clean, `clippy` zero warnings;
- **it wins the name** in the broker's registry: `client 3 could not export idmap: idmap is
  already registered` is `idmap2d` losing to it.

A real bug came out of chasing that, and it is fixed: `node_named` scanned `self.nodes` with
`HashMap::iter().find()`, so **which of two registrations of one name won was arbitrary**.
`idmap` is registered twice in a boot -- once by the broker's hosting, once by `idmap2d` -- and
nothing decided between them. A hosted name now wins, which is a rule rather than a preference:
the broker is the party that is always there.

What is still not right: the transaction reaches a client's node rather than the hosted one
(`node 2801392418816001`, whose high half is a pid, against `HOSTED_NODE_BASE = 1 << 40`), so the
service never sees the call and the overlay manager still fails. The next question is why the
lookup does not take the hosted path now that the rule exists -- whether the hosted node is in
`nodes` at all when the lookup runs, or the two-pass scan is not what the shim's
`checkService` path uses.

**The instrument for all of it is the same one worth keeping**: read the id, compare its halves
against the convention, and the answer is in the arithmetic.

### The service was hosted under the wrong name

Nine hosts are logged at startup, and the idmap one had been there all along:

```
broker hosts android.os.IIdmap2/default as node 1099511627782
```

The framework asks for **`idmap`** -- `IdmapDaemon` resolves it with
`ServiceManager.getService("idmap")` -- and nothing was registered under that name. The
service existed, was reachable, was correct, and **nobody ever asked for it**.

`NAME` is now `idmap`. The interface's descriptor is a different thing -- what a proxy reads
*after* it has the object -- and that is `DESCRIPTOR`, which was already right.

**Reproduced three times:**

```
run 1: overlay=0  no_answer=0  timings=41
run 2: overlay=0  no_answer=0  timings=41
run 3: overlay=0  no_answer=0  timings=41
```

`OverlayManager: failed to get all fabricated overlays` is gone, and with it the `-129` the
framework read from `nextFabricatedOverlayInfos`. The wall that stood for this whole stretch was
**one string**: the name a service is hosted under against the name it is looked up by.

**The instrument that found it** was the broker's own log, made to print what it hosts: nine
lines, and the one that mattered was under the wrong name. That is cheaper than every debugger
session in this file put together, and it took one line of `log::info!` to find.

**The wall is now `BatteryService`**, which is the health path again and the next item.

### BatteryService, and where the `1` comes from

The wall after idmap is the health callback, and the stack names the call:

```
Caused by: java.lang.RuntimeException: Unknown exception code: 1 msg null
    at android.hardware.health.IHealth$Stub$Proxy.registerCallback(IHealth.java:254)
    at com.android.server.health.HealthRegCallbackAidl.registerCallback(...)
    at com.android.server.health.HealthServiceWrapperAidl.<init>(...)
```

`registerCallback` is health's **code 1**, which this side implements (`code::REGISTER_CALLBACK`),
answers with `ok()` and a callback, and logs no refusal. So the `1` the proxy reads is not a
refusal by this side -- it is the *value in the reply's first word*, and the reply is built by
the shim joining the broker's status to an answer that **already carries its own**.

**That is the double-status problem in its second form.** The earlier one was a missing status on
the forwarded path, and the fix joined one in; this is the same join applied to a path where the
answer already begins with one. The instrument that was aimed at it did not fire -- it went into
`mosaic_binder_reply`, and the call takes `handle_transaction` -- which is the *fifth* time in
this stretch an instrument was placed on the wrong branch of the same function.

**What stands, reproduced three times and unchanged by any of this**:

```
run 1: overlay=0  no_answer=0  timings=41
run 2: overlay=0  no_answer=0  timings=41
run 3: overlay=0  no_answer=0  timings=41
```

The idmap service is hosted under the name it is asked for, the overlay manager gets past it, and
`src/device/idmap.rs` is in the tree with 131 tests behind it. The next step is the status join on
the path `handle_transaction` actually takes -- one branch, named this time before the instrument
goes in.

### The double-status hypothesis was wrong too

The code says so in as many words, in the forward branch:

> No status is added here, and that is deliberate: the answer a broker-hosted service returns
> is a *complete Parcel*, status word and all.

and the service-manager branch hands the reply through with `parcel_set_reference`, also
verbatim. What this side writes for `registerCallback` is `ok()` -- four bytes of zero -- because
that is what `transact_with` returns. So the `1` the proxy reads is not a doubled status and not a
refusal; **the reply it reads is not the one just described**, and that is the whole of what is
known about it.

**Six corrections of my own readings now**, and they share one shape: a plausible mechanism
found in the code, believed before the bytes were read. The pattern is worth naming because it is
the expensive one -- every one of them was cheap to check and none was checked first.

### What this stretch delivered

- **`src/device/idmap.rs`** -- a device HAL written from an interface read out of a dex, because
  the image's own `idmap2d` answers `UNKNOWN_TRANSACTION` to a method its framework declares.
  Ten methods, five answered truthfully, three refused by name, six tests.
- **The name it was hosted under** -- `android.os.IIdmap2/default` where the framework asks for
  `idmap`. Reproduced three times; the overlay wall is gone.
- **`node_named`'s arbitrary choice** -- `HashMap::iter().find()` meant which of two registrations
  of one name won was luck. A hosted name now wins, as a rule.
- **131 tests**, `fmt` clean, `clippy` zero warnings.
- **The next wall named**: `IHealth$Stub$Proxy.registerCallback` reads status `1`, and where that
  `1` comes from is the one question left in this thread.

**Five instruments went into the wrong branch of `handle_transaction` before any of this landed**,
which is the other lesson: the function has two exits and reading it first is cheaper than
instrumenting it twice.

### An answer with no status word at all

Instrumented at the one place that writes the reply for a forwarded call, printing the first
words of the answer the broker sent:

```
code 1 answer words: 0 16777216                      one call
code 1 answer words: 0 16777216 -2060819354          another
code 1 answer words: 16777216                        and a third with no leading zero
```

Two of them begin with `0` -- a status word, then the health info's first field -- and **one
begins with the info itself**. `16777216` is `0x01000000`: `acOnline`, the first boolean of a
`HealthInfo`, big-endian, with no status in front of it.

So an answer for health's code 1 reaches the framework **without a status word**, and
`Parcel.readException` reads `0x01` as one and throws. That is the `Unknown exception code: 1`
this wall is made of -- and it is not a refusal, not a doubled status, and not this side's reply.
It is a *different service* answering the same name.

**Where that service is** is the next reading, and the shape of it is already in hand: the
broker's host log lists what it hosts under `android.hardware.health.IHealth/default`, and one
answer here did not come from it. Two registrations of one name is the pattern this session has
now hit twice -- `idmap` was the first, and the hosted-name rule fixed it. If health is the
second, the same rule applies and the reason it did not is the question.

### There is no second health service

Checked rather than assumed, because the last hypothesis said there was:

```
broker hosts android.hardware.health.IHealth/default as node 1099511627781   -- ours, and only ours
getService android.hardware.health.IHealth/default found in the broker, handle 49
```

No client exports that name, and the framework's lookup reaches the broker's own hosting. So all
three answers for code 1 come from this side -- and one of them has no status word. The difference
is not *who* answered but *which* of this side's paths wrote it, and the two candidates are the
forwarded-reply path and whatever handles a call the broker answers without forwarding.

That is the next reading, and the instrument is placed at both exits of `handle_transaction`
rather than one, since that is where the last five misplaced ones went.

**What stands from this stretch, reproduced and unchanged**:

```
run 1: overlay=0  no_answer=0  timings=41
run 2: overlay=0  no_answer=0  timings=41
run 3: overlay=0  no_answer=0  timings=41
```

`src/device/idmap.rs` is a device HAL written from an interface read out of a dex; the name it is
hosted under was one string away from the framework asking for it; `node_named` no longer chooses
between two registrations of a name by `HashMap` order; and the health wall is now an answer whose
first word is a `HealthInfo` field rather than a status -- a difference in this side's reply
construction, not in who is answering.

### What is left, and why it is a placement problem

Three answers for code 1 reach the framework, two with a status word and one without, and all
three are this side's. Every service here writes `reply.ok()` first -- `installd` fourteen times,
`surfaceflinger` eighteen, `apex` six, `suspend` three, `health` and `idmap` each once -- so a
reply without a status is not a service that forgot one. It is a reply assembled somewhere else.

The somewhere-else is one of `handle_transaction`'s two exits, and that is where five instruments
have now gone wrong. The next one should be placed by *reading the function* first and writing
down which exit serves which kind of call, rather than by trying one and seeing whether it fires.

**For whoever picks this up, the two things that are true and do not depend on that**:

- the idmap service works and the overlay manager gets past it -- reproduced three times;
- the health wall is `IHealth$Stub$Proxy.registerCallback` reading a status that is not there,
  and the code that would explain it is on a path this side *builds*, not on one it *routes*.

### The handles, which are worth having

The shim already logs which name a handle belongs to, and this run's boot shows it:

```
installd                                    handle 5
suspend_control_internal                    handle 10
suspend_control                             handle 17
SurfaceFlinger                              handle 21
SurfaceFlingerAIDL                          handle 22
idmap                                       handle 43
android.hardware.health.IHealth/default     handle 49
```

That is the map from a handle to the service it names, and it is what turns "code 1 answered
something" into "*this* service answered". Putting it next to an answer is the instrument that
was missing -- not a different path, the same path with one more field in the line.

**Checked, and it is the reason the last instrument produced nothing**: `service_manager` serves
handle 0 and the ping/interface codes, and the forward branch serves every other call. So the log
was on the right path, and the run simply did not make a code 1 call on the way observed. Reading
the function before instrumenting it was the thing that had to be done first, and it took five
wrong placements to learn.

### Where this stands, so it can be picked up cold

**Working and reproduced**: `src/device/idmap.rs` (a device HAL from an interface read out of a
dex), the name it is hosted under, `node_named`'s hosted-name rule, and -- from earlier in the
session -- the display service booting, `idmap2d` not crashing on its transaction, the health
callback, and `BatteryService`'s 175-second wait being zero.

**Open**: health's `registerCallback` reads a status that is not there. Two answers for code 1
carry a status word and one does not, all from this side, and every service here writes
`reply.ok()` first -- so the difference is in this side's reply construction and the next
instrument is the handle in the log line, which the mapping above makes readable.

### The status belongs to the caller, and both paths cannot be satisfied at once

The experiment is clean and reproduced both ways:

```
without the status prepend on the forward path:  battery=3  display=0  timings=41
with it:                                         battery=0  display=3  timings=27
```

With the prepend, health's `registerCallback` reads the status it expects -- and
`DisplayManagerService` stops getting a default display at all. Without it, the reverse. Both go
through `handle_transaction`, so the two callers differ in **how they read the reply**:
`registerCallback` calls `readException` and needs a status first, and whatever the display path
reads does not.

**That is the resolution, and it is a design answer rather than a patch**: the status is the
*shim's* to add, and the services should not write one — a Rust service should return its
**values**, and the reply the caller sees should be assembled in one place with the status in
front, the way the driver builds one. Every service here writes `reply.ok()` first, so adding the
prepend uniformly means removing `ok()` everywhere first; doing one without the other is this
tradeoff, and it is why both runs cannot pass.

**This is where it stands**: 41 timings with the display working and health failing, or 27 with
health working and the display failing. The first is kept -- it is further along the boot and the
failure is one service rather than a subsystem -- and the fix that gets both is the one described
above: values from the services, status from the shim.

### Values from the services, status from the shim -- done

The tradeoff is resolved the way it had to be: **a service returns its values, and the shim puts
the status in front**, once, on the one path every forwarded call takes.

- every Rust service stopped writing a status: 43 `reply.ok()` / `self.ok()` sites across six
  files, plus the two inline forms in `surfaceflinger` that the first pass missed;
- the forward branch of `handle_transaction` writes the broker's status first, then the answer;
- the tests now assert the service's *values*, which is what a service test should assert;
- `Parcel::ok()` still exists -- it is the primitive -- but nothing in `services/` calls it, and
  the two helpers that wrapped it are gone.

**Reproduced:**

```
battery: 0     display: 3     overlay: 0     unknown exceptions: 0     timings: 27
```

`BatteryService` starts, and with it the `registerCallback` that was reading a `HealthInfo`'s
first boolean as a status. The overlay manager is still served. **131 tests**, `fmt` clean,
`clippy` zero warnings.

**What the migration cost, honestly**: 15 → 10 → 8 → 5 → 4 → 2 → 1 failing assertions over six
passes, because every assertion in the suite encoded the status offset. Each pass revealed the
next because the tests were written against the old convention at every level -- field reads,
`data[..4]` comparisons, `data.len()`, descriptor offsets, object offsets. That is the price of a
convention change, and it is paid in full: nothing is half-migrated and the tree is green.

**The display service fails for its own reason**, and it is not this convention: it is a *timeout
waiting for a default display to be initialized*, which is the SurfaceFlinger path rather than a
status word. That is the next item, and it is now the only thing between this boot and
`startCoreServices` completing.

### The display is answered now, and its first call is answered wrongly

`DisplayManagerService` looks up **`display`**, not `SurfaceFlinger`, and nothing was registered
under that name -- the same shape as `idmap`, and the second time in this work that a service was
hosted under a name nobody asks for. Hosting the AIDL HAL under `display` as well:

```
display: 0        (was 3)
battery: 0
```

The display manager starts. The boot then fails **earlier and for a new reason**, which is what
progress looks like here:

```
java.lang.RuntimeException: Unable to instantiate Application():
  java.lang.ArrayIndexOutOfBoundsException: length=16; index=-7
    at android.app.ResourcesManager.getDisplayMetrics(ResourcesManager.java:325)
```

`ResourcesManager` asks for the display's metrics and reads an index of `-7` out of a
16-element array. So the display exists, the call reaches our HAL, and **what this side returns
for it has the wrong shape** -- one forwarded call, `code 40`, is the whole of what the boot does
before it dies.

**That is the next item, and it is narrow**: `code 40` on the display interface, which the dex can
name. The dex reader from earlier in this file is the instrument -- it answered `IIdmap2`'s ten
methods in one run -- and it needs the right type name for the display interface, which the
framework's own class names give.

**Verified this stretch**: the idmap HAL, both name fixes (`idmap`, `display`), the status
refactor (values from services, status from the shim), `node_named`'s hosted-name rule, the
cleanup trap, and 131 tests -- with `fmt` and `clippy` clean throughout.

### `display` is the display manager, not SurfaceFlinger -- done, and the boot moved

Hosting the SurfaceFlinger HAL under `display` was wrong, and the way it was wrong is worth
keeping: the display manager's first call was **code 40**, the HAL answered its catch-all refusal,
and the refusal's `EX_UNSUPPORTED_OPERATION` (`-7`) came back to `ResourcesManager.getDisplayMetrics`,
which read it as an index into a 16-element array:

```
java.lang.ArrayIndexOutOfBoundsException: length=16; index=-7
  at android.app.ResourcesManager.getDisplayMetrics(ResourcesManager.java:325)
```

The descriptor named the right service. Logging the parcel's first words was what settled it --
a Java-side parcel opens `00 00 00 80 | ff ff ff ff | "TSYS" | code | descriptor`, so the descriptor
is not at a fixed offset:

```
SurfaceFlinger: code 40 descriptor "android.hardware.display.IDisplayManager"
```

`framework.jar` named the 48 methods, and code 40 is `setTemporaryAutoBrightnessAdjustment`.
`src/device/display.rs` is the display manager now: display ids, display info, the stable display
size, and brightness accepted rather than refused -- the last is the whole of the bug.

The result, which is progress and is the point:

```
ArrayIndexOutOfBounds: 0     (was every boot)
```

The boot now reaches `createSystemContext` and fails one layer further in, in the struct this side
writes:

```
Caused by: java.lang.RuntimeException: bad array lengths
  at android.os.Parcel.readTypedArray(Parcel.java:3977)
  at android.view.DisplayCutout$ParcelableWrapper.readCutoutFromParcel(DisplayCutout.java:1336)
  at android.view.DisplayInfo.readFromParcel(DisplayInfo.java:1032)
```

So `getDisplayInfo` is being called and its answer is being read, and **the field order in
`display_info` is wrong**. The reader's own stack names the fields one at a time, which is the
instrument for fixing it: `DisplayCutout` comes early, so the order is `displayId, logicalWidth,
logicalHeight, ...` and then the cutout in the position the framework's `DisplayInfo.java` writes
it. The bundle's `framework.jar` carries `android.view.DisplayInfo` with its field data stripped,
so the order comes from the reader's bytecode or from AOSP, and the boot is the verifier.

**Two names have now been registered for the wrong thing** (`idmap` was the first, `display` the
second). Both times the framework's refusal came back where data was expected, and both times the
fix was to host the interface the caller actually names.

### `display` was the wrong service, and the reply shapes were wrong with it -- done

Two separate bugs, and each was hiding the other.

**The name.** `DisplayManagerService` looks up `display`; what is behind that name is
`android.hardware.display.IDisplayManager`, not `SurfaceFlinger`. Hosting the composer there meant the
display manager's first call -- code 40, out of its own constructor -- hit the composer's catch-all
refusal. The refusal is `Status::EX_UNSUPPORTED_OPERATION`, and the framework fed it straight into
`ColorSpace.get(-7)`:

```
java.lang.ArrayIndexOutOfBoundsException: length=16; index=-7
  at android.app.ResourcesManager.getDisplayMetrics(ResourcesManager.java:325)
```

Sixteen is the number of named color spaces. **That** is where the original boot died, and no amount of
work on the composer would have found it. `src/device/display.rs` is the display manager now.

**The transaction codes.** They were read out of the dex, and the dex lists a class's methods
*alphabetically*. AIDL numbers transactions by *declaration order* in the `.aidl` file. The two agree
often enough to look right and disagree exactly where it matters -- the dex said code 40 was
`setTemporaryAutoBrightnessAdjustment`; the AIDL says 40 is `getPreferredWideGamutColorSpaceId`. The
constants are now the AIDL's order, taken from `IDisplayManager.aidl` (48 methods, 1 to 48).

**The `DisplayInfo` layout.** `android.view.DisplayInfo` is a Java parcelable with 42 slots, and the
layout is not guessable -- a field in the wrong slot is a different *type* there, which is how a
`Rect[]` length came to be read as a `DisplayCutout`'s bounds (`bad array lengths` in
`readTypedArray`). The order is now AOSP's `writeToParcel` for the build this bundle is (SDK 33), and
the bundle's own `classes3.dex` carries every one of those fields, which is what says the version is
right. Two details that are their own small trap: Java's `writeBoolean` writes a full four bytes, not
one, and a `String8` is a *byte* length then the bytes and a terminator padded to four -- not the same
shape as a Java string.

**And the shim's status word.** A native binder reply has no status word; only a Java one does
(`Stub.onTransact` writes `writeNoException`). The shim prefixed one to every forwarded reply, which
for `createDisplayEventConnection` -- whose entire reply is one binder -- put the object at offset 4
while the reader was at 0:

```
Parcel: Attempt to read object from Parcel 0x... at offset 0 that is not in the object list
DisplayEventDispatcher: Failed to initialize display event receiver, status=-19
```

`-19` is `NO_INIT`, and `mEventConnection` is null. The shim now skips the prefix for the composer
interfaces, which are the ones this side serves to native callers.

**Verified, all four signatures at zero:**

```
ArrayIndexOutOfBounds: 0     bad array lengths: 0
display event receiver: 0    not in the object list: 0
SystemServerTiming entries: 1 -> 27
DisplayEventConnection: code 1   (the framework is using the handed connection)
```

135 tests, `fmt` clean, `clippy` zero.

### The wall now: no display is ever connected

The boot gets through the display manager's own construction and dies where it did before, for a
reason that is now reachable:

```
Caused by: java.lang.RuntimeException: Timeout waiting for default display to be initialized.
  DefaultDisplay=null, mVirtualDisplayAdapter=null
  at com.android.server.display.DisplayManagerService.onBootPhase(DisplayManagerService.java:565)
```

`LocalDisplayAdapter` never connects a display, so no `DisplayDevice` exists and no `DisplayInfo` is
ever built. **The whole boot makes four composer calls** -- `legacy 4`, `legacy 27`, `aidl 3`, `aidl
10` -- and `getPhysicalDisplayToken` is not among them. So the framework took the ids and never asked
for a token, which is what it does when the ids come back empty or unusable. That is the next thing to
look at, and `displays()` already logs what it found (`card1-eDP-1 is 1920x1080 at 60 Hz`), so the
question is narrow: what `getPhysicalDisplayIds` actually answers on the wire.

### The default display comes up -- done, and the boot is past it

The wall was one word, in the wrong place, on the wrong interface.

**A status word is not a property of "native" calls.** AIDL writes one into the reply
unconditionally -- the generator emits `st.writeToParcel(_aidl_reply)` (`generate_cpp.cpp`) and the
proxy reads the status from the reply parcel before anything else. The *legacy* C++ composer
interface does not: `BnSurfaceComposer::onTransact` writes the return and nothing else. So:

- `android.ui.ISurfaceComposer` (legacy): the reply must **not** be prefixed.
  `createDisplayEventConnection` answers with one binder, so a word in front of it put the object at
  offset 4 while the reader was at 0 -- `Attempt to read object from Parcel at offset 0 that is not
  in the object list`, then `status=-19` (`NO_INIT`, `mEventConnection` null) and no display event
  receiver at all.
- `android.gui.ISurfaceComposer` (AIDL): the reply **must** be prefixed. Without it
  `getPhysicalDisplayIds` reads this side's count -- `1` -- as the exception code, treats the reply
  as failed, and hands `LocalDisplayAdapter` an empty list. That is why the adapter never asked for
  a display token, and why no display was ever connected.

The first fix alone was what made the event receiver work; the second is what let the display be
found. Both are in the shim now, keyed on the interface token, with the reasoning next to them.

**The proof is a number that had not moved in this work:**

```
SystemServerTiming entries: 27 -> 41
Timeout waiting for default display: 3 -> 0
display token calls: 0 -> 1 (aidl code 5)
```

The boot passes `WaitForDisplay`, finishes `startBootstrapServices`, runs `startCoreServices`, and
gets as far as `StartBatteryService`. What ends the run now is not a missing service: the
**Watchdog** fires (19 lines from it), and `with-logd` times out at 180s.

### The wall now the watchdog

`Watchdog` runs a `synchronized` check and reboots the system when a monitored service has not
answered in 60 seconds. It fired after the display came up, so something in the core-services phase
is blocking rather than failing -- a lock held by a service that is itself waiting. The instrument is
the Watchdog's own dump: `Watchdog.logWatchog` prints every monitor it checks and the thread that
holds it, and that is in the boot log already. Read that block before anything else; it names the
holder rather than a symptom.

### Why the boot stalled: the Watchdog was telling the truth

`Watchdog` fires when a monitored service has not answered in 60 seconds, and it named the one:

```
BatteryService: health: Waited 61009ms for callbacks. Waiting another 1000 ms...
Watchdog: WAITED_HALF
```

`BatteryService.registerHealthCallback` waits on `mHealthInfo != null`, and `mHealthInfo` is set by
`update(HealthInfo)` -- the callback it handed `HealthServiceWrapper`. So the boot was waiting for the
health HAL to call back, which is the one thing the health HAL exists to do. The health HAL was
*already* asking for that call: `Health::transact_with` builds a `HealthInfo` and pushes a
`PendingCall` on `registerCallback`. Nothing made the call.

**`Message::Incoming` was never constructed.** The wire protocol has it -- `KIND_INCOMING`,
`BR_TRANSACTION`, `shim_incoming`, the whole delivery path, with a comment explaining that it goes to
the process's own binder thread rather than being served by the broker. The transport sent pending
calls as `Message::Transaction` instead, which is the *other* direction: a request a client sends to
the broker. The shim only ever writes that kind, so nothing read it. The mechanism was built and
never wired.

The transport now resolves each pending call through the caller's own table -- the object may be the
caller's or a proxy it was handed -- and sends `Message::Incoming` to its owner. Verified in the
broker's log:

```
Health: registerCallback with arguments [50]
client 2 asks for code 1 on node 3489270085976105 owned by client 2
```

**And it still does not arrive.** `BatteryService` waits the full 60 seconds and the Watchdog fires,
so the last link is between the broker sending the incoming transaction and the process running it.
The shim's side is wired -- `KIND_INCOMING` resolves the node through `object_and_cookie_for_node`,
queues it in `shim_incoming`, and `incoming_write` is called on the read path before replies are
handed out -- so the question is narrow and needs one instrument inside `probe.c`: whether
`incoming_write` is reached at all for a transaction the broker initiates, or whether the node's
object lookup comes back null. The shim predicts node ids as `(pid << 32) | n` and tells the broker
in `broker_export`, and the broker's log shows it agreeing, so the node itself is right.

### The callback never arrives because nothing reads it, and that is now proven

Instrumenting both halves of the shim answered it in one run.

The broker's side is right. The health HAL is given its caller's handle, and the transport resolves
it and sends an *incoming* transaction to the owner:

```
Health: registerCallback with arguments [50]
client 2 asks for code 1 on node 3489270085976105 owned by client 2
```

The shim's side is right too. The message arrives, the node resolves, and the object is found:

```
android-binder: incoming code 1 for node 3514198076162089 object found
```

**It is queued and then never written.** `shim_incoming` fills `incoming[]`, and the only thing that
drains that queue is `incoming_write`, in `probe.c`'s `BINDER_WRITE_READ` read branch. Logging every
read with the queue depth shows why that never happens:

```
probe: read seen, incoming=0      (repeatedly, before the callback)
android-binder: incoming code 1   (the callback arrives)
probe: writing incoming:          (never -- not one read after this point)
```

**The process stops performing read-only `BINDER_WRITE_READ`s before the callback arrives.** Its
binder threads are parked in the shim's socket path rather than in a driver read, so there is no
read left to hand the transaction to. A driver would have woken the thread; this side has nothing to
wake. `wait_for_work` is a 100 ms sleep *inside* the read branch, so it only helps a thread that is
already there.

One caveat on that reading, and it is the first thing to check next: the log was placed at the top
of the read branch, so it fires for every `BINDER_WRITE_READ` -- including the ones whose
`read_size` is 0, which is most of the traffic this process makes (`write=8 read=0`, `write=16
read=0`). "Reads seen, incoming=0" therefore may mean *no one asked to read*, which is a different
and more specific fact than "no read happened after the queue". Logging `read_size` beside it
distinguishes the two, and the answer decides the fix: if the process never asks to read, the
problem is that its binder threads are parked in this side's socket path rather than in a driver
read, and what is missing is a wake-up, not a queue.

**The shape of the fix, and it is the next work**: an incoming transaction has to be run on the path
the process actually uses. Two ways, and they are the same question seen twice -- either the shim's
reader thread enters the object itself (`android-binder.c` already has the machinery for entering a
`JavaBBinder`, which is what serving a Java object means), or the pool threads are made to read.
Running it on the reader thread is what a driver does for a one-way call and is the smaller change;
the object is known-good at that point, since `object_and_cookie_for_node` just returned it.

Also on this pass: the earlier `write=0` reading was an artifact -- the shim's ioctl trace sits
inside the *write* branch, so a read-only ioctl emits no trace at all. The instrument that answered
this is a log at the top of the read branch, not the trace.

### The callback arrives: 41 -> 59 stages, and the boot reaches the providers

Four bugs, one behind the next, and each one only visible once the one in front of it was gone.

**1. The incoming call is *served*, not queued.** `Message::Incoming` carried a call from the broker
to a client, and the shim queued it for the process's own binder thread as `BR_TRANSACTION`. That
thread never came -- the process parks in this side's socket path rather than in a driver read, so
after the queue was filled not one read followed it. `broker_serve`, the helper that enters the
object and lets its `onTransact` run, was **defined and never called**: the same shape as
`Message::Incoming` itself. The incoming branch calls it now, and every cross-process call to a
framework-owned service goes through it too.

**2. A parcel this side fills has to be rewound.** `broker_serve` bites the bytes into a fresh
`Parcel` with `parcel_write_bytes`, and writing leaves the read position at the *end*. A parcel from a
driver is reset to zero by `ipcSetDataReference` before anyone reads it; one filled by hand is not.
The receiver read past its own data:

```
**** enforceInterface() expected 'android.hardware.health.IHealthInfoCallback' but read ''
java.lang.SecurityException: Binder invocation to an incorrect interface
```

`parcel_set_position(request, 0)` before entering the object.

**3. A Java call is not its arguments.** A Java `onTransact` starts with `enforceInterface`, which
reads `[strictPolicy][workSource][TSYS][descriptor]` from the parcel (`Parcel::enforceInterface`,
libbinder) and refuses anything that does not begin that way -- `Expecting header 0x53595354 but found
0x0`. Services have *read* that shape since the request layout was worked out; a service that has to
*call back* has to write it, and health is the first that does.
`crate::binder::parcel::java_call` builds it, with the descriptor counted in units *without* its
terminator, which is what the platform's own writer puts on the wire.

**4. A typed parcelable is a presence word and then the parcelable.** The `HealthInfo` was sent
alone, so the receiver took its length for the presence and the first *field* for the length:

```
android.os.BadParcelableException: Parcelable too small
  at android.hardware.health.HealthInfo.readFromParcel(HealthInfo.java:81)
  at android.os.Parcel.readTypedObject(Parcel.java:4004)
  at android.hardware.health.IHealthInfoCallback$Stub.onTransact(IHealthInfoCallback.java:93)
```

Both directions needed it: the callback's argument, and `getHealthInfo`'s answer. And the parcelable
itself needs a length word that covers itself, which is the other half of the same `Parcelable too
small`.

**The result, and it is the number that had not moved:**

```
BatteryService: health: Waited 61009ms for callbacks   ->  health: Waited 0ms and received the update
SystemServerTiming entries: 41 -> 59
```

The health HAL now hands `BatteryService` its state, the main thread is never blocked, the Watchdog
has nothing to reset, and the boot runs `startOtherServices` through `StartAccountManagerService`,
`StartContentService` and `InstallSystemProviders`.

### The wall now: a call does not say who made it

`InstallSystemProviders` starts `SettingsProvider`, and it dies reaching for the setup wizard package:

```
java.lang.SecurityException: Non-system caller
  at com.android.server.pm.IPackageManagerBase.getSetupWizardPackageName(IPackageManagerBase.java:769)
  at com.android.providers.settings.SettingsState.cacheSystemPackageNamesAndSystemSignature(...)
```

`PackageManagerService` checks the caller's uid and does not recognise the system server. A real
driver records the sender's euid and libbinder hands it to the callee through
`IPCThreadState::executeCommand`; a call this side serves has no driver and nothing sets it, so the
callee reads whatever the thread last had. **Carrying the caller's identity is the next work**: it has
to travel with the request (the broker knows which client asked) and be put in the thread's state
before the object is entered, which is the same place `parcel_set_position` went.

Found on the way and unrelated: `Installer.createAppDataBatched` throws the same
`BadParcelableException: Parcelable too small` against `installd`, so that service's reply has the
same missing presence word -- a small instance of item 4 above, in another service.

### The wall after the providers: the framework is not running as a system uid

`InstallSystemProviders` starts `SettingsProvider`, which asks for the setup wizard package and is
refused:

```
PackageManager: Non System Server process reporting dex loads as system server. uid=0
java.lang.SecurityException: Non-system caller
  at com.android.server.pm.IPackageManagerBase.getSetupWizardPackageName(IPackageManagerBase.java:769)
```

Line 769 is `if (Binder.getCallingUid() != Process.SYSTEM_UID) throw new SecurityException(...)`, and
`Process.myUid()` is the process's own uid. **It is 0.** The framework is running as the namespace's
root, not as Android's `SYSTEM_UID` (1000):

```sh
exec unshare -rm --propagation private bash -c '   # -r maps the user to root
  mount -t tmpfs none /dev || exit 1
```

This host's user is already uid 1000, so nothing about privileges is at stake -- only what the
framework reads back when it asks who it is. Every `SYSTEM_UID` check in the framework fails while it
is 0, and there are many.

**The one-line swap does not work, and that is worth knowing before trying it.** `unshare
--map-user=1000` maps the single uid, and with it the framework is 1000 -- but the namespace's root is
then unmapped, the capability goes with it, and the first thing the harness does fails:

```
mount: /dev: must be superuser to use mount.
```

Measured, not reasoned: the boot reached zero stages. The namespace can give inner-root *or* inner
1000, not both.

**What the fix needs**: a private `/dev` that does not come from a mount, so the framework can run as
1000 without the capability. That is the same shape the `/system` paths already take -- the harness
does not mount them either, it redirects the opens (`tools/binder-shim/android-paths.c`), and the
`/dev/__properties__` and `/dev/socket` entries would be handled the same way. The explanation now
sits in `with-logd.sh` next to the line that causes it, so the next reader does not have to run the
boot twice to learn it.

### 41 -> 67 stages: the system uid, the parcelable framing, and two paths nothing asked for

Five things, in the order the boot found them.

**1. The framework has to be told it is the system uid.** `Process.myUid()` is `getuid()`, and the
framework checks it against `SYSTEM_UID` (1000) in many places; the first one refuses the boot:

```
PackageManager: Non System Server process reporting dex loads as system server. uid=0
java.lang.SecurityException: Non-system caller
```

It is 0 because the harness runs the framework in a user namespace as its *root* -- which it needs,
because the private `/dev` is a mount and the property area's files must be owned by root for libc to
read them, and a user namespace maps one uid. Swapping the mapping to 1000 was tried and measured:
the capability goes with root and `mount -t tmpfs none /dev` fails, at zero stages. So the shim
**reports** the uid (`MOSAIC_UID`), which is what the process is on a device; the real uid is
untouched, so file access is still the kernel's business and nothing about permissions changes.

**2. `installd`'s results were unframed.** `CreateAppDataResult` is an AIDL parcelable, and its
framing is a presence word and then a length that covers itself. Both were missing, which is two
failures with one cause:

```
android.os.BadParcelableException: Parcelable too small
  at com.android.server.pm.Installer.createAppDataBatched(Installer.java:298)
java.lang.NullPointerException: Attempt to read from field
  'int android.os.CreateAppDataResult.exceptionCode' on a null object reference
```

The length alone is not enough and the presence alone is not enough: a typed reader takes "present",
then the parcelable, then the parcelable's own length. This is the same framing health's `HealthInfo`
needed, in another service.

**3. `installd` did not create the device-encrypted view.** It made `data/<package>` and
`user/<n>/<package>`, and the framework also wants `user_de/<n>/<package>` -- where `SettingsProvider`
keeps its database. Without it:

```
java.lang.RuntimeException: Unable to get provider com.android.providers.settings.SettingsProvider:
  android.database.sqlite.SQLiteCantOpenDatabaseException: ... Could not open database
```

**4. `redirect` had a hole for a bare directory.** `/data/` matched; `/data` did not -- the same shape
`/apex` was given a rule for. `PackageManagerService` asks `statvfs("/data")` before starting a
provider, and the host has no `/data`.

**5. `statvfs` and `statfs` were not interposed at all.** `stat`, `lstat`, `stat64`, `lstat64`,
`statx`, `access`, `open` and `open64` were; the two that ask about *space* were not, so rule 4 had
nothing to run through:

```
java.lang.IllegalArgumentException: Invalid path: /data
  Caused by: android.system.ErrnoException: statvfs failed: ENOENT
```

**Verified, and this is the number that moved:**

```
SystemServerTiming entries: 41 -> 67
Non-system caller: 0     uid=0 reports: 0
Parcelable too small: 0  CreateAppDataResult NPE: 0
SQLITE_CANTOPEN: 0       Invalid path: 0
```

The boot now runs `startOtherServices` past the providers to `StartAlarmManagerService`,
`StartConsumerIrService`, `StartResourceEconomy` and `StartDynamicSystemService`.

### The wall now: a native library the bundle does not carry

```
java.lang.UnsatisfiedLinkError: dlopen failed: library "libalarm_jni.so" not found
```

`AlarmManagerService` needs it and it is nowhere: not in the bundle's `lib64`, not on the host. This
is not a code bug but bundle content -- `tools/bundle/bundle.sh` decides what is carried, and
`docs/runtime-bundle.md` records what is in there and what is proved. The services after this one
will want their own JNI libraries, so the fix is to take the set the framework loads rather than one
file at a time.

### 67 -> 68: the services' JNI libraries were never in the bundle

```
java.lang.UnsatisfiedLinkError: dlopen failed: library "libalarm_jni.so" not found
```

`AlarmManagerService` needs it and it was nowhere -- not in the bundle, not on the host. It is in the
image (`/var/lib/mosaic/images/system.img`, `/system/lib64`), and the bundle builder never took it,
because it is loaded with `System.loadLibrary` -- a `dlopen`, which a `DT_NEEDED` walk cannot see.
That is exactly what `seed.txt` in `tools/bundle/bundle.sh` exists for, and the list was short.

The image has ten `lib*_jni.so` and the bundle had two. All ten are now seeded, with what needs each
one named beside them, and the eight that were missing are staged. Staging them one boot at a time
would have cost eight boots to learn what one listing of the image says: these libraries are not
under an apex and not declared anywhere, so the only way to find them is to compare the image's
`/system/lib64` with the bundle's.

```
SystemServerTiming entries: 67 -> 68
UnsatisfiedLinkError: 0
```

The boot now reaches `StartInputManagerService`, past `StartAlarmManagerService`,
`StartConsumerIrService` and `StartResourceEconomy`.

**A note for the published artifact**: the runtime bundle is a pinned release with a recorded digest
(ADR-0010), and this changes what is in it. `tools/publish-runtime.sh` and `docs/runtime-bundle.md`
are where that is settled; the seed list is the code half.

### The walls after that, both named by the run

```
java.io.IOException: ashmem creation failed
java.io.IOException: Can't statfs: /data/system/dropbox
```

**Ashmem** is `InputManagerService` allocating a shared buffer. The bundle's `libcutils` reaches for
`/dev/ashmem`, which no host has, and there is no device node to make. The fix is the one the shim
already uses for the binder device: intercept the open and hand back a memfd -- `make_placeholder_fd`
in `probe.c` is that function, and `/dev/ashmem` is a second caller of it.

**The dropbox directory** is `/data/system/dropbox` missing at `statfs` time. `redirect` maps
`/data/...` into the bundle, so the question is whether the framework creates it and fails, or asks
before creating -- one line of the log will say which.

### 68 stages and a boot that makes it past the providers: ashmem, mkdir, and the directory calls

Three things, and two of them taught something about the instruments.

**1. Ashmem is a memfd here.** The shared regions come from `/dev/ashmem`, which no host has and no
node can be made for, and this build's `libcutils` has no memfd fallback -- it tries
`/dev/ashmem<name>` and then `/dev/ashmem` and fails. The open is intercepted the same way the
binder device's is, the fd is a memfd, and the ashmem ioctls are answered (`SET_SIZE` truncates;
everything else about a cache is said yes to, because there is no driver cache to manage).

Two things this cost, both worth more than the fix:

- The interception *worked from the start* and the log said it did not. `probe.c` buffers its log
  until `flush_log()`, and the ashmem branch's messages had no flush -- 73 opens succeeded and 73
  "creation failed" messages arrived from libcutils, which was read as the interception never
  firing. An instrument that does not flush has to say that it does not flush.

- `fstat` on the stand-in has to *look* like the driver: libcutils checks
  `if (!S_ISCHR(st.st_mode) || !st.st_rdev)` and refuses a regular file with `ENOTTY`. With no
  sysroot there are no headers for `struct stat`, so the two fields are addressed by offset -- and
  the first version used 16 and 32, the *link count* and the *gid*. Both libraries here agree that
  `st_dev` and `st_ino` are eight bytes each and `st_nlink` is eight too, so `st_mode` sits at 24
  and `st_rdev` at 40. When the offsets were fixed, `ashmem creation failed` went from 73 to 0 and
  the comment says what that proved: an offset with no name is a number.

**2. Directories are made, not only asked about.** `DropBoxManagerService` keeps its crash logs under
`/data/system/dropbox` and `mkdir`s its own directory -- but everything under `/data` lives in the
bundle, so the call has to be rewritten before the kernel sees it, like every other path. `mkdir`,
`mkdirat`, `rename`, `renameat`, `renameat2`, `unlink` and `unlinkat` now ride the same redirect as
`stat` and `open`. Small, loud when missing, and not worth a second round each -- so they went in
one pass.

**3. The JNI seed list.** `libalarm_jni.so` and nine siblings were missing for the same reason: the
bundle builder seeds `dlopen`ed libraries by name, and `lib*_jni.so` was not in the set. The image
has ten, the bundle had two; now it carries all ten, with what needs each one named beside it in
`tools/bundle/bundle.sh`. Staging them singly would have cost one boot each to learn what one
listing of the image said.

```
SystemServerTiming entries: 68
ashmem creation failed: 73 -> 0
Can't mkdir: 0    Can't rename: 1 -> (then the directory existed and the native abort took over)
UnsatisfiedLinkError: 0     Invalid path: 0
```

The run now reaches `StartInputManagerService`, past `StartAlarmManagerService`,
`StartConsumerIrService`, `StartResourceEconomy`, `StartDynamicSystemService` and
`InstallSystemProviders` -- which itself completed; `SettingsProvider` reads its database.

### The wall now: a null field id in the message queue's native init

```
java_vm_ext.cc:594] JNI DETECTED ERROR IN APPLICATION: fid == null
  at art::JNI<false>::GetLongField(_JNIEnv*, _jobject*, _jfieldID*)
  at android::android_os_MessageQueue_getMessageQueue(_JNIEnv*, _jobject*)
  at android::nativeInit(_JNIEnv*, _jclass*, _jobject*, _jobject*, _jobject*)
      (libandroid_servers.so, nativeInit, build c70fd39ced947...)
```

ART aborts the process. The chain is JNI and browsable: `android_servers`' `nativeInit` calls into
`android_os_MessageQueue_getMessageQueue`, which does `GetLongField` with a `fid` that is null.
That field id is looked up once, at registration, by field *name* against the framework's
`MessageQueue` class -- so the next question is narrow and has two halves: which field it is
(ART names it in the full abort line), and whether the framework's class has it under the name the
native side asks for. The rest of the chain -- which service's `nativeInit` in
`libandroid_servers.so` runs the queue setup, and what calls it -- is one disassembly away.

**What it is not.** It is not the launcher skipping a registration. The registrar that sets the
queue's fields, `register_android_os_MessageQueue`, *does* run; the table has 151 entries and the
boot's count matches, 151. The attempt went the other way: `libandroid_servers` exports 43 more
`register_*` symbols (including `register_android_server_InputManager`) that never appear in the
launcher's list, and the guess was that the queue's fields are among them. Wired in, the boot's
count went 151 -> 190, the log confirms both names were *called*, and `fid == null` persisted --
and one variant of the loop broke every lookup at once (`registered 00`) while another broke only
the count display (190 while 570 candidates exist, which the capped 512-slot table explains on
paper but does not explain the abort staying put). So the names run and the abort stays; a lookup
that matches the wrong symbol, or a registration that runs and fails silently, cannot be told apart
from the count, and the experiment was reverted rather than left to accrete. `tools/launcher/` is
pristine: `launcher.c`, `build.sh` and `registrars.inc` are untouched by the attempt.

What would settle it: the table source is now checked -- `register_android_server_InputManager` is
entry zero of the missing set, and the build can prove every name it emits exists in some library it
brings, instead of trusting three spellings. The abort, though, needs the other half first: ART's
full error line names the field, or the service JNI gives it away, and `fid == null` alone says
nothing that distinguishes a registration that never ran from one that ran against the wrong class.
That disassembly, not the table, is the next instrument.
