Objective
- Migrate mosaic-rust from a Rust port of Waydroid's LXC container model to a Wine-like native execution model: Android apps run as host processes, with Android APIs, Binder and HAL translated to Linux, no container and no kernel binder driver.
- Immediate practical goal: get com.android.server.SystemServer to boot and eventually run an arbitrary app (Termux as the test fixture).
Requirements
- "do it in this repo only" — the pivot happens in mosaic-rust, not a new repo.
- "move to the new" — retire the container model; do not keep it as a second supported mode.
- "choose best" / "choose best and optimal" for: Bionic-vs-glibc strategy, broker transport, first-milestone scope, ADR handling ("up to u").
- Earlier explicit choice: full rename — later scoped so host-facing identity is mosaic and guest-facing identifiers stay Waydroid's for compatibility.
- "implement all phases one by one, remove not needed code".
- "fix all", "fix next", "continue fix", "fix till the system works", "list me all the things left to do and complete to achieve my desired system", "implement A", "fix all of A", "fix A remaining".
- Test on a real app: "download/termux and fix all issue".
Decisions
- ADR-0001: native per-process execution, no container.
- ADR-0002: reuse AOSP ART and Bionic; reimplement only framework services/Binder/HAL.
- ADR-0009: Bionic is the app process libc (no glibc shim — they cannot coexist); only broker and helper are glibc Rust.
- ADR-0004: userspace Binder, no kernel driver.
- ADR-0005: one lazily started, idle-exiting unprivileged broker (wineserver analog); install is a two-step transaction so the broker is never root.
- ADR-0008: narrow privileged helper (polkit-scoped uid-helper mode) for UID allocation; the install command drives the prompt because a systemd user service has no session.
- ADR-0010: runtime bundle published as a pinned, hash-verified artifact.
- ADR-0012: Bionic processes run through the Android linker with a bundle-local ld.config.txt; the namespace holding libc.so must be the one ART looks up by name (namespace.default.visible = true, visible is the exporting key — there is no is_exported).
- ADR-0013: userspace property service; area provisioned once by the privileged step (/dev/__properties__ is hardcoded in libc).
- ADR-0014: Binder via a preloaded Bionic shim, not CUSE (root-only, would mean writing the driver ABI twice).
- ADR-0015: reuse the Android Java framework (framework.jar already executes); replace only the device layer.
- Runtime bundle is a separate build pipeline/artifact, not built in this repo's CI.
- One binary with subcommands (mosaic daemon, mosaic uid-helper), not two binaries.
- No NDK needed: clang --target=x86_64-linux-android21 -shared -fPIC -nostdlib builds Bionic shared objects.
Work State
Completed
- P0 — container model deleted (LXC, 15 container actions, kernel-binder interfaces, 4 session services, 10 helpers, src/guest.rs, LXC configs, D-Bus/systemd container units); native skeleton (broker + registry + framed socket protocol + idle exit, runtime bundle fetch, userspace-binder seed, uid-helper); packaging retargeted (user units, polkit action, deps = polkit + xz); ADRs 0001–0015, CONTEXT.md, docs/plan.md, docs/remaining-work.md. 37 tests, clippy -D warnings, fmt green.
- P1 — runtime bundle builds from a system image with one command: Android linker, library closure, boot classpath, ICU data, etc/public.libraries.txt, all *-res.apk, fonts*.xml, 208 fonts, vendor/etc/public.libraries.txt (empty; absence is fatal to SystemConfig). ART prints ART version 2.1.0 x86_64; a DEX executes; Termux 0.118.3 compiles to a 9 MB OAT with 14,225 methods (bundle.sh compile <bundle> <apk> speed).
- P2 — property area implemented from AOSP formats (property_info trie + prop_area, prop_bt BST ordered by length-then-bytes); app_process64 loads libandroid_runtime.so, registers JNI natives, initialises Build.VERSION/RuntimeInit.
- A1 font map ✅ — cause was that the shim redirected open but not stat/access.
- A7 paths ✅ — /vendor, /product, /system_ext, /odm redirected; vendor/etc/public.libraries.txt supplied.
- A8 properties ✅ — write path implemented in the shim; failing property was cache_key.is_compat_change_enabled (34 chars vs libc's 32-char limit); over-long names accepted, not stored.
- A2 registry — implemented in tools/binder-shim/android-binder.c (store local binder from addService, hand back on getService/checkService), but never exercised: services fail during start and never reach our addService.
- Launcher (tools/launcher/) interposes JNI_CreateJavaVM, calls registrars in AOSP order (from registrar_order.txt), resolves 151 candidates across libraries that provide them (plain extern "C" name plus both C++ manglings).
- Harness (tools/bundle/with-logd.sh) provides logd, property service, path redirection, MOSAIC_ANDROID_ROOT, timeout (MOSAIC_TIMEOUT) and output cap (MOSAIC_MAX_OUTPUT), a preload-existence guard, and builds its own missing preload artifacts.
- All work committed and pushed through 94f0ec2 (also 122dca6, db8b29f, 4d39dc3, e589e9f, c44225e, 755e517, f890eda, 684c75a, 6fe6fdc, 527a3c1, a9c1ab7).
Active
- SystemServer.startBootstrapServices now starts a succession of services: StartFileIntegrityService, StartInstaller, StartIStatsService, StartPowerStatsService (the previously failing one), DeviceIdentifiersPolicyService, UriGrantsManagerService.
Blocked
- HAL registration: framework registers HALs with hwservicemanager (HIDL, /dev/hwbinder) and servicemanager (AIDL). Neither answers — HidlServiceManagement: getService: defaultServiceManager() is null, Cannot register HIDL android.frameworks.stats@1.0::IStats: -38, Cannot register AIDL android.frameworks.stats.IStats/default: -129, then statsd aborts (SIGABRT). Needs a second device (/dev/hwbinder) with its own service manager in the shim.
- A3 refcounts, A4 fd passing, A5 broker transport — deferred by dependency: all three exist to make binder work between processes, and everything is still one process.
- A6 privileged step — half done: harness stand-ins only (androidSetThreadPriority, TaskProfiles::SetTaskProfiles, setpriority, sched_setaffinity, pthread_setschedparam). The real systemd LimitNICE and provisioning code is not written and needs root to verify.
- RLIMIT_NICE is 0 on this host and cannot be raised even by root in a user namespace (raising the hard limit needs privilege in the initial user namespace).
- mosaic runtime fetch still uses a placeholder channel (https://ota.waydro.id/mosaic, 404s).
Next Move
1. Implement a second device in the shim: /dev/hwbinder with its own service manager, so the framework can register HALs.
2. Finish A6: systemd LimitNICE on the broker unit plus property-area/path provisioning script.
3. Implement A3–A5 (refcounts, SCM_RIGHTS, broker socket transport with per-process handle tables) once a second process exists.
4. Re-run SystemServer and confirm services register, exercising A2's registry.
5. Publish the runtime bundle as a pinned artifact and point runtime fetch at it.
Relevant Files
- /home/tenzin/development/mosaic-rust/docs/remaining-work.md: the full remaining-work list, dependency-ordered, updated with the current wall.
- /home/tenzin/development/mosaic-rust/docs/plan.md: phase plan (P1–P10), gates, effort, kill criteria.
- /home/tenzin/development/mosaic-rust/docs/binder.md: measured /dev/binder surface, why the driver framing was abandoned, the API-level route.
- /home/tenzin/development/mosaic-rust/tools/binder-shim/android-binder.c: registry + BpBinder::transact interposition (A2).
- /home/tenzin/development/mosaic-rust/tools/binder-shim/probe.c: path redirection (open/stat/access), binder device interception, logging.
- /home/tenzin/development/mosaic-rust/tools/binder-shim/android-properties.c: __system_property_set implementation (A8).
- /home/tenzin/development/mosaic-rust/tools/binder-shim/pretend-nice.c: harness stand-ins for priority/cgroup/affinity calls (A6 evidence).
- /home/tenzin/development/mosaic-rust/tools/bundle/bundle.sh: bundle build; classpaths derived from the image's own configs; resource/font/vendor collection.
- /home/tenzin/development/mosaic-rust/tools/bundle/with-logd.sh: harness (MOSAIC_ANDROID_ROOT, MOSAIC_PRELOAD, MOSAIC_PROPERTY_DIR, MOSAIC_TIMEOUT, MOSAIC_MAX_OUTPUT).
- /home/tenzin/development/mosaic-rust/tools/bundle/make-property-area.py: property area + property_info generator (catch-all prefix for runtime properties).
- /home/tenzin/development/mosaic-rust/tools/bundle/android-property-service.py: property write service (appends new properties, persists index).
- /home/tenzin/development/mosaic-rust/tools/launcher/launcher.c: JNI_CreateJavaVM interposition, registrar registration, class launch.
- /home/tenzin/development/mosaic-rust/tools/build-native.sh: builds all Bionic artifacts.
- /var/lib/mosaic/images/{system.img,vendor.img}: build inputs (ext4; read with debugfs, no root).
- /tmp/opencode/phase3/: working bundle used for all measurements (has lib64/, framework/, etc/, fonts/, vendor/etc/, bootclasspath.txt, systemserverclasspath.txt, run.sh).
Important Context
- Everything runs without root via unshare -rm (private /dev); the product needs one privileged step for /dev/__properties__, LimitNICE and the Android paths.
- Drive the ratchet by running: each failure names the next gap. SystemServer reports progress via SystemServerTiming: markers in the log.
- Log capture: a Bionic process logs to /dev/socket/logdw and drops messages when nothing listens; with-logd.sh provides a listener, which is how every gap was found.
- with-logd.sh refuses to start when MOSAIC_PRELOAD names a missing library, and builds artifacts itself — three runs were wasted on that pattern before.
- ART commits what -Xmx asks for; a 1 GB heap fails the compiler's arena mapping (Failed anonymous mmap(...): Out of memory) while 8 GB is free. 256 MB works.
- /tmp is a 7.7 GB tmpfs; two runaway runs wrote ~5 GB logs before the output cap existed.
- Reply construction facts from drivers/android/binder.c (binder_thread_read): a BR_REPLY carries the answered transaction's code and flags, with data.ptr.buffer/offsets addressing the read buffer.
- BpBinder's handle offset is unknown (IBinder derives virtually from RefBase); only handle 0 has been reachable.
- Launcher registrars: libhwui defines register_android_graphics_classes as extern "C" and it is the one that registers android.graphics.Typeface; libandroid_runtime merely imports it.
- dex2oat needs explicit paths only — no properties, no namespace, no root — making bundle.sh compile a clean end-to-end check.
- Running a real app (Termux) is not achievable yet: it needs the activity lifecycle, ActivityManager, a surface, and it executes a Linux userland from its bootstrap payload.
- Tier 4 (Play services, attestation, DRM) is permanently out of scope; SELinux cannot be enforced on a non-Android host (UIDs + Landlock/seccomp instead).
- Repo is public; remote origin is git@github.com:yesheytenzin/mosaic.git.
