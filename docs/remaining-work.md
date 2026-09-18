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

With those, there are no fatal signals at all, `StartActivityManager` is passed,
and the boot stops somewhere else: `ActivityManagerService`'s constructor hangs
inside `ProcessStatsService` -> `ProcessStats.<init>` ->
`Debug.getDirtyPagesPid` -> memtrack -> `AServiceManager_checkService` (the NDK
path) -> the shim's `service_manager` -> `Parcel::flattenBinder`, blocked on an
ART `MemMapArenaPool` mutex. `flattenBinder` calls `IPCThreadState::self()`, and
reaching it from inside a binder transaction on the NDK path is what deadlocks.
The `svc`-style service the NDK path looks up is `memtrack.proxy`, which the shim
does hand back; the question is how the NDK's `checkService` reply differs from
the Java one's.
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
