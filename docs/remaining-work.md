# Remaining work

What is left before Mosaic runs an arbitrary Android app, in dependency order.
It is a working list: the near-term items each have a gate that can be checked by
running something, because that is how every phase so far has been settled.

Working today: the runtime bundle builds from an image in one command; ART runs a
DEX and AOT-compiles a real app; the framework starts and `SystemServer` completes
`PlatformCompat`, `ReadingSystemConfig`, `startBootstrapServices` and
`StartWatchdog`; properties are read and written; a userspace binder answers
`checkService`, `getService` and `addService` at the `libbinder` API level; 151 JNI
registrars resolve; the shim presents Android's absolute paths.

## A. The critical path, next

1. **Font map** ✅ — `stat`/`access` were not redirected, so the font parser could
   not see the fonts. Fixed; `SystemServer` moved on.
2. **Binder registry.** Store the binder `addService` carries, hand it back on
   `getService`, route a transaction to a handle by calling the stored
   `BBinder::transact` (exported). Also `listServices`, `isDeclared`, and
   `BpBinder`'s handle offset, which is still unknown because only handle 0 has
   been reachable. *Gate:* a service registered by name is found by name and a
   transaction reaches it.
3. **Reference counting and lifetime.** `BC_ACQUIRE`/`RELEASE`/`INCREFS`/`DECREFS`
   and death notification, so a held binder stays alive. *Gate:* a binder survives
   while referenced and is cleaned up after.
4. **Descriptor passing.** `SCM_RIGHTS` for `Parcel` file descriptors. *Gate:* a
   transaction carrying an fd arrives intact.
5. **Broker transport.** Move binder out of the in-process shim to the broker over
   its socket (ADR-0011, ADR-0014), with per-process handle tables. *Gate:* a
   transaction between two processes works.
6. **One privileged step (ADR-0013).** `LimitNICE` on the broker's unit, the
   property area, and the paths a product presents instead of redirecting.
   *Gate:* `Process.setThreadPriority` works without the harness stand-ins.
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

`startBootstrapServices` starts `StartFileIntegrityService`, `StartInstaller`,
`StartIStatsService`, `StartPowerStatsService`, `DeviceIdentifiersPolicyService`
and `UriGrantsManagerService`. The next wall is HAL registration: the framework
registers its HAL implementations with `hwservicemanager` (HIDL, over
`/dev/hwbinder`) and with `servicemanager` (AIDL), and neither answers --
`defaultServiceManager() is null` for HIDL, and the registrations fail with -38
and -129, after which statsd aborts the process.

So the next work is a second device: `/dev/hwbinder` needs its own service
manager in the shim, the way `/dev/binder` already has one.

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
