# Implementation plan: running any Android app

This is the plan for the whole product, not for the next phase. It is grounded in
what has already been built and measured, and it names the things that are
unknown rather than smoothing them over.

Its central claim is in [The spine](#the-spine-reuse-the-platform-replace-the-device):
Mosaic does not reimplement Android. It reuses the Android platform as far up as
it is portable and replaces the device layer underneath it — the same shape as
Wine, and the same principle as ADR-0002, applied one layer higher than before.

## What "any app" means

"Any app" is not a specification. This is what Mosaic will aim at, in tiers, with
the last tier explicitly out of scope.

| Tier | Definition | Examples |
| --- | --- | --- |
| 1 | Pure-Java or Kotlin apps, standard widgets, no native libraries, no Google services | calculators, notes, readers, small games |
| 2 | Tier 1 plus bundled x86_64 native libraries, graphics, audio, files | media players, emulators, Termux-class tools |
| 3 | Tier 2 but ARM-only native libraries | most apps shipped before 2020, many games |
| 4 | Anything requiring Google Play services, hardware attestation, or DRM | banking apps, paid streaming, most games with anti-cheat |

Tier 4 is **out of scope and always will be**, not because it is hard but because
it depends on proprietary services Mosaic cannot ship and on attestation that a
non-Android system cannot honestly provide. This should be stated in the app
compatibility list rather than discovered by users.

The plan's target is: **all of tier 1, most of tier 2, and tier 3 once the ARM
translator lands.** The compatibility matrix measures that, rather than "it works
for us on Termux".

## The spine: reuse the platform, replace the device

The single most important fact discovered so far, and the one the plan rests on:
**the Android Java framework is already in the runtime bundle and already
executes.** `framework.jar` — `ActivityThread`, `ActivityManager`,
`PackageManager`, `WindowManager`, the resources system, the view hierarchy — is
present, and the framework's static initialisation runs (`Build.VERSION`,
`RuntimeInit`) up to the first Binder call.

So the work splits cleanly:

**Reused, unmodified:**

- ART (runtime, verifier, JIT, AOT compiler) and Bionic, from the image.
- The framework's Java: `framework.jar`, `ext.jar`, the `com.android.server.*`
  service implementations, the widget toolkit, `AssetManager`'s resource format.
- The apps themselves, untouched.

**Replaced, because the host is not an Android device:**

| Android component | Mosaic's replacement |
| --- | --- |
| kernel binder driver | userspace binder behind a preloaded shim (ADR-0004, ADR-0014) |
| `init` (property service, boot) | the broker, plus one privileged provisioning step (ADR-0013) |
| `zygote` | the broker spawning app processes directly |
| `system_server` | the broker, running the real `com.android.server.*` classes |
| SurfaceFlinger, gralloc, hwcomposer | per-app Wayland toplevels and the host's GPU driver (ADR-0006) |
| AudioFlinger | PipeWire |
| `installd`, `vold` | the broker, plus the UID helper (ADR-0007, ADR-0008) |
| SELinux enforcement | real UIDs plus Landlock/seccomp, with the gap stated below |
| input flinger | the Wayland compositor's input, delivered as Android input events |

Everything in the right column is a *device*, not a *platform*. That is the
distinction the plan turns on.

## The ratchet: measure the gap by running it

The reason to believe this is tractable, and the reason the previous phases
insisted on real artifacts: **every missing piece announces itself.**

`com.android.server.SystemServer` is a `main` in `framework.jar`. Run it on our
ART and it fails at the first thing the platform does not provide:

```
dalvikvm -cp framework.jar com.android.server.SystemServer
→ UnsatisfiedLinkError: no implementation found for android.os.MessageQueue.nativeInit
```

Implement that one native, run it again, get the next one. The metric is not "how
much of Android have we written" — it is **how far into system_server's boot does
it get**, which is a number that only goes up and that can be checked in CI. The
same ratchet applies to an app process: run `ActivityThread` with a real app, fix
the first missing native, repeat.

This is why the plan does not contain an estimate for "reimplementing the
framework": that work is not reimplementation, it is a measured sequence of gaps,
each one discovered by execution.

## Phases

Each phase ends in a gate that can be checked by running something, because that
is how the first three phases were done and it is the only reason to trust them.

### P1 — Runtime bundle ✅

Bundle built from a system image by one command; ART reports its version; a DEX
executes. `docs/runtime-bundle.md`.

### P2 — App process and properties ✅

`app_process64` loads `libandroid_runtime.so`, registers JNI natives, initialises
the framework, and runs `am` until its first Binder call. The property area is
generated from AOSP's formats. `docs/runtime-bundle.md`.

### P3 — Userspace Binder

*Goal:* an app process can talk to the service manager.

*Work:* settle `BINDER_WRITE_READ`'s framing (open, see `docs/binder.md`); route
transactions to handle 0 through a service registry; reference counting
(`BC_ACQUIRE`/`BC_RELEASE`/`BC_INCREFS`/`BC_DECREFS`); handles for services that
live in another process, over the broker's socket; death notification.

*Gate:* `am` asks the service manager for `activity` and gets a well-formed answer
(an error reply is fine while no service exists — it must not crash or hang).
Then: a service registered by our own test process is found by name from an app
process, and a transaction reaches it.

*Risk:* the framing, which is currently the blocker. Low once solved.

### P4 — The system server, running the real services

*Goal:* `com.android.server.SystemServer` boots far enough that `ActivityManager`
can start an activity.

*Work:* the ratchet above, against `SystemServer` and the services it starts:
`PackageManagerService`, `ActivityManagerService`, `WindowManagerService`,
`ActivityTaskManagerService`, `WindowManagerGlobalLock`, the `installd` and
`vold` equivalents. Each needs natives from `libandroid_runtime.so` and the
Java-side platform pieces; some need real host facilities (filesystem layout,
package scanning, permission database).

*Gate:* `SystemServer` reaches the end of its boot and `am start` returns without
error for an app with no Activity (a service-only app). The number of boot stages
reached is tracked in CI.

*Risk:* **the highest in the plan.** It assumes the Java services can run without
a real platform underneath. The cheapest way to falsify it is a two-day spike:
run `SystemServer` on today's runtime and count how many natives it needs before
it stalls in a way a shim cannot fix. If that number is unmanageable, the fallback
is reimplementing the minimum service surface in the broker instead — a much
larger job — and the plan changes.

**The spike has been run, and it went well.** The real
`com.android.server.SystemServer`, out of the image's own `services.jar` (10.5 MB
`classes.dex` plus 8.3 MB `classes2.dex`), loads and runs on today's runtime:

```
at android.os.Binder.getNativeBBinderHolder(Native Method)
  at android.os.Binder.<init>(Binder.java:657)
  at com.android.server.SystemServer$SystemServerDumper.<init>(SystemServer.java:703)
  at com.android.server.SystemServer.<init>(SystemServer.java:480)
  at com.android.server.SystemServer.main(SystemServer.java:650)
```

It gets into the constructor, creates a Binder, and stops on **one** native:
`android.os.Binder.getNativeBBinderHolder`. That is registration done by
`libandroid_runtime.so`, which `dalvikvm` does not load but `app_process` does.
Run through `app_process` instead and the flow proceeds past that point to
`ProcessState` and the Binder driver.

So the gap is a list of natives plus the device layer, not a wall — the assumption
this phase rests on now has evidence rather than optimism. Two consequences:

- **The launcher matters, and now exists.** `tools/launcher/` is a Bionic shared
  object that interposes `JNI_CreateJavaVM`, calls the 150 exported `register_*`
  functions that `AndroidRuntime::startReg` calls internally, and then runs a
  class. Order matters and cost a round: calling them alphabetically runs
  `os_Binder` before `util_Log`, and Binder's class initialisation touches
  `StrictMode`, which calls `android.util.Log.isLoggable` — registration then
  aborts on a native that would have been registered a few lines later. The order
  is now AOSP's own, extracted from `AndroidRuntime.cpp` into
  `tools/launcher/registrar_order.txt`.

With the launcher, **SystemServer boots**:

```
launcher: registered 150 native registrars
launcher: running com.android.server.SystemServer
SystemServerTiming: InitBeforeStartServices took to complete: 0ms
libc: Using old property service protocol ("ro.property_service.version" is not set)
```

That is the real system server running its own boot sequence, with the
framework's natives registered, no crash, and its first block completed. It then
stops, and it stops **before reaching Binder at all** — no `BINDER_WRITE_READ`
ever happens. The line about the old property service protocol is the clue: it
comes from libc's property *write* path, and Mosaic implements property *reads*
(the mapped area) without implementing the service properties are written to
(`/dev/socket/property_service`, which ADR-0013 assigns to the broker).

`SystemServer` writes properties in its very first block — `SYSPROP_START_COUNT`,
`persist.sys.timezone` — and then does `RuntimeInit.setDefaultApplicationWtfHandler`
before `StartServices`. That is where it stops.

The property service is now implemented (`tools/bundle/android-property-service.py`),
from bionic's own protocol: a 128-byte `prop_msg` on a SOCK_STREAM connection at
`/dev/socket/property_service`, acknowledged by closing. Writes to properties the
area defines are applied in place — the generator records where each value lives
so the writer needs no trie walk — and a write to an undefined property is
reported and dropped, because adding one means changing an area that running
processes have mapped.

With it, `SystemServer` gets further:

```
SystemServerTiming: InitBeforeStartServices
SystemServer: persist.sys.timezone is not valid (UTC); setting to GMT.
SystemServer: Entered the Android system server!
SystemServerTiming: InitBeforeStartServices took to complete: 24ms
```

That middle line is the property service working: the server read the property,
decided it was invalid, wrote a new one, and the write landed
(`property-service: set persist.sys.timezone='GMT' -> applied`). It then stops
before `StartServices` begins, without a Binder call and without an abort.

So the next question is what it does between `InitBeforeStartServices` and
`StartServices` — `RuntimeInit.setDefaultApplicationWtfHandler` is the only call
there — and why it ends quietly rather than failing. Then the binder framing,
which gates the services themselves.

A measurement was taken with a missing launcher and looked like a stall for a
reason that did not exist; `tools/build-native.sh` now builds every Bionic
artifact in one command, so a cleaned output directory cannot silently produce a
wrong reading again.

### P5 — Windowing and graphics

*Goal:* an app draws in a real Wayland window.

*Work:* replace `libgui`/`SurfaceFlinger` with a Wayland-backed implementation of
the native window API that `libhwui` (Skia) and `libandroid` bind to: buffers
(dma-buf), EGL, and the `Surface`/`SurfaceControl` plumbing. `android.view`'s
`ViewRootImpl` then renders through the host GPU driver into a toplevel.

*Gate:* a Tier 1 app with one Activity opens a Wayland toplevel, draws its UI, and
responds to touch. Screenshot compared against the same app on a device.

*Risk:* Skia and `libhwui` are large and assume gralloc semantics. Mitigation:
software rendering (`libhwui`'s CPU path) first to prove the windowing, then GPU.

### P6 — Install and launch, for real

*Goal:* `mosaic install app.apk` then `mosaic launch` starts a real app.

*Work:* APK parsing (manifest, splits, signatures, `resources.arsc`), DEX and
native library placement, per-app UID and data directory (ADR-0007/0008),
`/data/data/<pkg>` layout, odex/vdex generation, permission records, and the
intent/launch path through the services from P4.

*Gate:* install and launch a Tier 1 app that does not exist on the test machine
beforehand, from the CLI, with its data surviving a restart.

*Risk:* package management is where "any app" starts to bite — split APKs, ABI
selection, uses-library, signatures. Bounded but wide.

### P7 — Input, audio, clipboard, notifications

*Goal:* an app is usable, not just visible.

*Work:* input events from the compositor into `InputManager`'s Java side; audio
through PipeWire; clipboard; notifications via the desktop's notification service
(ADR-0006's "native toplevels" logic extends here).

*Gate:* type into an app and hear it play a sound.

### P8 — ARM translation

*Goal:* apps whose native libraries are ARM-only run.

*Work:* integrate FEX-Emu or box64 as the ARM ABI path in `libnativebridge`,
which ART already knows about — `ro.dalvik.vm.native.bridge` exists and
`libnativebridge.so` is in the bundle. That is the hook: Mosaic provides a native
bridge, rather than writing a translator.

*Gate:* a Tier 3 app with only `armeabi-v7a` libraries runs.

*Risk:* performance and correctness of the translator; out of Mosaic's control,
which is why it is deferred.

### P9 — Storage, media, and the long tail

*Goal:* the things apps notice when they are missing.

*Work:* `MediaCodec` (software codecs through the host's libraries),
`MediaPlayer`, camera (v4l2), location (host geolocation), sensors, Bluetooth,
`WebView` (a Tier 4-scale component on its own — likely a permanent gap).

*Gate:* a media app plays a video file.

### P10 — Packaging, security, and release

*Goal:* a user installs Mosaic and an app.

*Work:* the pinned runtime bundle published as a release artifact (ADR-0010),
packages for at least one distro, the polkit-provisioned property area and UID
range at install time, documentation, and the compatibility list.

*Gate:* install Mosaic on a clean machine, install an app, run it, uninstall both
cleanly.

## Cross-cutting: the security model, honestly

Android's isolation is UIDs *plus* SELinux plus a locked-down kernel. Mosaic can
have the first and not the second: SELinux policy cannot be enforced on a host
that is not running Android's policy, and pretending otherwise would be worse
than the gap.

What the plan commits to instead (ADR-0007, and a new ADR for the rest):

- real per-app system UIDs, as Android does;
- `landlock` and `seccomp` on each app process, for filesystem and syscall
  boundaries;
- no-new-privileges, and no setuid binaries inside an app's reach;
- the honest statement that a malicious app is stopped from reading another app's
  data, but not from doing something Android's SELinux would have caught.

This is a real reduction in guarantee and belongs in the documentation, not in a
footnote.

## Verification

The plan is built so that progress is machine-checkable, because the alternative
is a compatibility list maintained by hand.

- **Compatibility matrix.** Tier 1: 20 apps chosen small and offline. Tier 2: 15
  with native libraries and graphics. Tier 3: 10 ARM-only. Each row is a script
  that installs, launches, performs a scripted interaction, and checks the result.
- **Ratchet counters.** `SystemServer` boot stages reached; natives implemented;
  the number of apps in the matrix that pass. These go into CI output so a
  regression is a number that went down.
- **The real-app checks already built.** `bundle.sh compile` on a real APK proves
  the runtime end to end with no framework; the `am`-reaches-binder check proves
  the framework boots to a known wall. Both are cheap and stay.
- **Test fixtures.** A hello-world APK built by us. `d8` is a Java jar, and we
  have an ART to run Java jars on, so the fixture can be built by the thing under
  test — which is also a nice self-check.

## Effort and sequencing

Ranges, in person-months of focused work, with the honest caveat that P4 and P5
dominate and that P4 has an unknown that could change everything.

| Phase | Effort | Parallel? |
| --- | --- | --- |
| P3 binder | 1–2 | yes, with P4's spike |
| P4 system server | 6–12 | no, it is the spine |
| P5 windowing | 4–8 | yes, once P4 has AMS |
| P6 install/launch | 2–4 | yes, after P3 |
| P7 input/audio | 2–3 | yes |
| P8 ARM | 1–2 | yes |
| P9 media/long tail | unbounded | partially |
| P10 packaging | 1–2 | yes, throughout |

The order that gets to a visible win soonest: **P3 → the P4 spike → P5 with
software rendering → P6**, which would put a Tier 1 app on screen. P4's spike
should happen before committing to any of it.

## Kill criteria

Three findings should stop the plan and force a different one, and each has a
cheap test:

1. **`SystemServer` cannot boot on a shimmed platform.** Test: the P4 spike. If
   the required native surface is unbounded rather than a list, this architecture
   does not work and the alternative is a much larger reimplementation.
2. **`libhwui`/Skia cannot render to a Wayland buffer without gralloc semantics.**
   Test: P5's software path.
3. **The property and binder interfaces cannot be provided without root.** Test:
   provisioning `/dev/__properties__` and `/dev/binder`'s replacement through the
   privileged helper on a clean machine. Both are currently assumed to be one
   root step at install time.

## Immediate next steps

1. Fix `BINDER_WRITE_READ`'s framing from the parcel side, and reply to handle 0.
2. Stand up the service registry so a registered service can be found by name.
3. ~~Run the P4 spike.~~ **Done** — `SystemServer` runs and stops on one native.
   ~~Build the launcher.~~ **Done** — `tools/launcher/` registers the framework's
   natives and runs a class; `SystemServer` now stops one gap further on. Next:
   ~~Make the launcher report which registrars fail~~ **Done**, and with the
   right registration order `SystemServer` boots to `InitBeforeStartServices`.
   It now waits on item 1, the binder framing, which is where it was always
   going to wait.
4. Build the hello-world fixture APK (via `d8` on our own ART) and add it to the
   verification set.
5. Add the compatibility matrix skeleton and run the first three Tier 1 apps.
