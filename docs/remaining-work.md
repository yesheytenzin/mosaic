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
   verifiable here; `RLIMIT_NICE` cannot be raised from a user namespace.
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
service manager:

```
SystemServerTiming: StartWatchdog ... StartInstaller ... UriGrantsManagerService
SystemServerTiming: StartPowerStatsService
SystemServerTiming: StartIStatsService          (no longer aborts)
SystemServerTiming: MemtrackProxyService
SystemServerTiming: StartActivityManager
android-binder: AIDL register memtrack.proxy
android-binder: checkService memtrack.proxy found
```

and then it stops, waiting for a service that does not exist:

```
Installer: installd not found; trying again        (once a second, for good)
```

`installd` is a native daemon on a device, and the service manager is telling the
truth. This is not a binder problem any more. Two things follow, and they are one
piece of work:

1. Implement the AIDL interface in Rust and host it in the broker, so the shim
   hands back a handle to something that answers.
2. Forward a lookup the shim cannot answer to the broker over its socket. That is
   what `src/binder/transport.rs` already does between two connections; the shim
   has to become a client of it.

### Why it was lost, and what was done about it

The Java path used to get nothing from the service manager, for two reasons that
both looked like "the registry does not work":

- **The interface token is not first in the request.** Twelve bytes precede it and
  an int32 sits between it and the name, so the name came out empty and every
  lookup was a lookup for `""`. Searching for the descriptor, and then for the
  first printable string after it, is what fixed it. `docs/binder.md` has the
  layout and the constants, read out of libbinder's machine code rather than
  recalled.
- **An empty answer to a waiting thread is not neutral.** libbinder reads the next
  command out of the buffer it was handed, and an empty buffer reads as command 0:
  `*** BAD COMMAND 0 received from Binder driver`. `BR_NOOP` is the reply that
  means "nothing happened", and a waiting thread gets that.

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
