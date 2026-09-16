Here's the complete remaining work, grounded in what this session actually proved (bundle builds, ART runs, framework boots to the point of needing IPC) and what each failure has named. Nothing here is speculative — the items with a gate are things we can check by running.
Where you are now
Working: bundle from an image in one command; ART loads, verifies, AOT-compiles a real app (Termux: 14,225 methods); app_process64-equivalent framework startup; properties (read and write); the userspace binder at the API level answering checkService/getService/addService; 151 JNI registrars; the framework inside SystemServer.run past the binder.
Current wall: the system font map comes out empty, so Typeface.getDefault() returns null.
A. The immediate critical path (next six steps)
1. Empty system font map. SystemFonts parses the config we staged and produces nothing. Find whether the path it opens is the file we staged and whether its entries resolve. Gate: Typeface.getDefault() non-null; SystemServer.run(907) passes.
2. Binder registry. Store the binder addService carries, hand it back on getService, route a transaction to a handle by calling the stored BBinder::transact (exported). Also listServices, isDeclared, and the handle offset on BpBinder (currently masked because only handle 0 is reachable). Gate: a service registered by name is found again by name, and a transaction reaches it.
3. Reference counting and lifetime. BC_ACQUIRE/RELEASE/INCREFS/DECREFS, death notification (link, unlinkToDeath), so a client holding a binder keeps it alive. Gate: a service's binder survives while referenced and is cleaned up after.
4. Descriptor passing. SCM_RIGHTS for Parcel file descriptors, over the broker socket. Gate: a transaction carrying an fd arrives intact.
5. Broker transport. Move binder from in-process shim to the real broker over its socket (ADR-0011/0014), with the framed protocol and per-process handle tables. Gate: a transaction between two processes works.
6. RLIMIT_NICE and property provisioning. One privileged step: the broker unit gets LimitNICE, and the property area plus /dev-equivalent paths are provisioned (ADR-0013). Gate: Process.setThreadPriority succeeds without the harness stand-in.
B. system_server to completion (P4 — the big one)
 7. Every service it starts after PackageManagerService: AMS, WMS, ATMS, WindowManagerGlobalLock, ActivityTaskManager, StorageManager, RoleManager, PermissionManager, ConnectivityService, Statsd, AlarmManager, JobScheduler — each will announce what it needs, as the fonts did.
 8. PackageManagerService's inputs: /system/etc/permissions/*.xml, /system/etc/sysconfig/*.xml, /data/system/packages.xml, /data/dalvik-cache, /system/etc/privapp-permissions-*.xml, shared-library lists, the framework-res idmap overlays.
 9. installd and vold equivalents — the native side of package installation, data directory creation, and cache management, which PMS calls over binder.
10. /data layout as Android expects: system/, misc/, dalvik-cache/, data/<pkg>/, with the right ownership.
11. Property service completeness: a real store that persists (persist.*), accepts new properties, and serves reads to other processes.
12. A boot image (or accept imageless): current startup is imageless and slow; building boot.art from the boot classpath would cut startup and match device behaviour.
13. selinux-free equivalents of what init does: ueventd-ish device permissions, /dev/binder-equivalent, /dev/ashmem-equivalent if apps use it.
C. Running an app process (P6)
14. The launcher becomes the app entry point: per-app classpath, ActivityThread as the start class, JNI library loading, and the app's Application/Activity lifecycle.
15. APK installation: manifest and binary-XML parsing, split APKs, signature verification, resources.arsc, ABI selection, uses-library.
16. Per-app UID and data directory (ADR-0007/0008): the privileged helper, useradd, chown, reserved range; /data/data/<pkg> and its lib/, cache/, files/.
17. DEX optimisation: vdex/odex generation on install, then reuse.
18. The broker starts app processes on demand, tracks them, and reaps them.
19. Startup time: a pre-warmed template process (zygote-like) once the basics work — optional, but real apps notice 3-second starts.
D. Make it visible (P5) and usable (P7)
20. Windowing (ADR-0006): replace libgui/SurfaceFlinger with a Wayland-backed native window implementation: Surface, SurfaceControl, BLASTBufferQueue, SurfaceTexture, dma-buf buffers.
21. EGL and the GPU: EGL via the host driver, libhwui/Skia rendering (software path first, then GPU), Vulkan for games.
22. Input: compositor input into InputManager's Java side; keyboard, pointer, touch.
23. Audio: AudioFlinger replacement over PipeWire; AudioTrack/AudioRecord.
24. Clipboard, notifications, IME, Intent resolution to host handlers (open URL, share).
25. Multi-window/surface details: SurfaceView, overlays, and Android's window policy in the broker.
E. Compatibility breadth (P8–P9)
26. ARM translation (ADR-0003): FEX-Emu or box64 behind libnativebridge — the hook already exists (ro.dalvik.vm.native.bridge, libnativebridge.so in the bundle).
27. Media: MediaCodec (software codecs), MediaPlayer, MediaExtractor, camera (v4l2), recording.
28. Sensors, location, Bluetooth, NFC as host equivalents where they exist.
29. WebView — a Tier-4-scale component; likely a permanent gap unless you ship Chromium's Android WebView.
30. DRM/widevine, Play Integrity, GMS — out of scope, permanently.
F. Productisation (P10)
31. Publish the runtime bundle as a versioned, hash-pinned artifact and implement mosaic runtime fetch against it (the current channel URL 404s).
32. Packaging for at least one distro (deb/Arch exist but need retargeting to the new layout), the systemd user units, the polkit action.
33. mosaic install/launch/uninstall/query end-to-end, plus desktop entries generated per app.
34. Security model, stated honestly: UIDs + Landlock + seccomp on each app process; no SELinux; documented as a reduction.
35. Man pages, install docs, and the compatibility list (tier, status, known gaps) rather than letting users discover them.
36. Upgrade path: bundle and app migration when the runtime version changes.
G. Cross-cutting infrastructure
37. Decide the path strategy: the shim redirects /system, /data, /apex; the product should either keep that or present the paths (mount namespace/symlink at install). One of the two, documented.
38. /proc fidelity: apps read /proc/self/*; some expect Android-specific entries.
39. system_properties for other processes (the socket service is per-harness today).
40. Logging: ship the logd stand-in as a real service, or accept hybrid logging.
41. The ratchet as a CI gate: counters for registrars, boot stages, binder calls, and apps passing; regressions must show as numbers going down.
42. Test fixtures: hello-world APK; a scripted compatibility matrix.
H. Effort and order
Block	Range	Notes
A (critical path above)	2–4 weeks	mostly known, each item small
B (system_server)	6–12 person-months	the spine, and the biggest unknown
C (app execution)	2–4 person-months	after B
D (visible/usable)	4–8 person-months	windowing dominates
E (compatibility)	2–4 months, then unbounded	media and WebView are the long tail
F (product)	1–2 person-months	parallel to everything
Shortest path to a Tier-1 app on screen: A → the essentials of B (PMS + AMS + WMS enough to start one activity) → D's software-rendering windowing → C. That ordering is worth protecting: it gets something visible long before system_server is complete.
I. What will never work
Google Play services, Play Integrity/attestation, Widevine L1, anti-cheat, and anything that requires Android's SELinux to be enforced. I'd put these in the compatibility list at the top rather than have users discover them.
Two honest notes. First, B is not a checklist so much as a ratchet — the items are discovered by running, which is why each of this session's steps named itself. Second, the single highest-value thing you could do for the project isn't on this list: finish the binder registry (A2), because everything in B, C, and D depends on the framework's services being reachable. Until then the framework can boot but not function.
