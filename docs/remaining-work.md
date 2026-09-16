# Remaining work

What is left before Mosaic runs an arbitrary Android app, in dependency order.
It is a working list: the near-term items each have a gate that can be checked by
running something, because that is how every phase so far has been settled.

Working today: the runtime bundle builds from an image in one command; ART runs a
DEX and AOT-compiles a real app; the framework starts and `SystemServer` completes
`PlatformCompat`, `ReadingSystemConfig`, `startBootstrapServices` and
`StartWatchdog`; properties are read and written; a userspace binder with per-process handle
tables, reference counting, descriptor passing and a broker transport is served by
the daemon (reached by the broker's own callers, not yet by the framework); 151 JNI
registrars resolve; the shim presents Android's absolute paths.

## A. The critical path, next

1. **Font map** ✅ — `stat`/`access` were not redirected, so the font parser could
   not see the fonts. Fixed; `SystemServer` moved on.
2. **Binder registry.** The authority is `src/binder/broker.rs`, served over the
   socket by `src/binder/transport.rs`, and it is exercised: a name resolves to a
   node and an owner, handles are per process, a transaction to another process is
   forwarded and its answer relayed, and a process cannot publish a node that
   belongs to another pid. What is *not* done is the shim forwarding to it, and
   behind that a harder fact: the Java path cannot reach any of it, because
   `BinderProxy.transact` resolves inside libbinder and no preload can see it.
   Those calls land at `ioctl`, whose framing is now settled (see `docs/binder.md`)
   and whose reply is the next thing to write. *Gate:* a service registered by
   name is found by name and a transaction reaches it -- met for the broker's own
   callers, not yet for a Java one.
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
   Bionic compiles in. A test keeps the two limits from drifting. *Gate:* not
   verifiable here -- `RLIMIT_NICE` is 0, a process cannot raise its own hard
   limit, and a user namespace does not help. `make verify-priority` is the check
   for whoever has root, and what it establishes is in `docs/todo-a.md`.
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

`ActivityManagerService`'s constructor still does not complete. Two known reasons,
in order:

1. ~~The registry stores a pointer without a reference.~~ Fixed: the object is
   taken with `Parcel::readStrongBinder`, whose reference is left in place, so the
   registry owns the service from registration on. Verified -- no SIGSEGV and no
   staleness guard, with lookups still finding what is registered.
2. The broker client works up to the last layer, and is off by default. Verified
   against a running broker, with the framework in one process and
   `tools/two-process-call.py` in another:

   - the shim publishes what it registers (a frame per registration, `published
     memtrack.proxy`)
   - the second process resolves a name published by the first
     (`handle=1 node=0x8e3600000002 owner=1`)
   - the broker forwards the call to the owner (`the broker sent a transaction`)

   What does not happen is the owner *serving* it: `broker_serve` never reaches
   its own log line, so the call is received and not answered, and the broker
   eventually gives up on the connection. The two candidates are the Parcel built
   over the bytes the sender wrote, and running a Java-side binder's `transact`
   on a thread ART does not know about -- `memtrack.proxy` is registered from
   native code, but the same path serves Java objects later.
3. Then `installd`, which does not exist and which the framework waits for:

```
Installer: installd not found; trying again
```

Three shim bugs were fixed to get here, all recorded in `docs/binder.md`: the
binder object type constants (the old ASCII encoding matched nothing, so no
registration was ever kept), the string16 padding (two bytes for an odd count, not
four), and writing a null strong binder by writing nothing (handing libbinder null
is a SIGSEGV inside the framework's `getService`).

## B. `system_server` to completion

9. Every service after `PackageManagerService`: AMS, WMS, ATMS, `StorageManager`,
   `RoleManager`, `PermissionManager`, `ConnectivityService`, `AlarmManager`,
   `JobScheduler` — each announces what it needs, as the fonts did.
10. `PackageManagerService`'s inputs: `/system/etc/permissions`, `/system/etc/sysconfig`,
    `/data/system/packages.xml`, `/data/dalvik-cache`, `privapp-permissions`,
    shared-library lists, and the resource overlays.
11. `installd` and `vold` equivalents — package installation, data directories and
    cache management, which PMS calls over binder.
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
