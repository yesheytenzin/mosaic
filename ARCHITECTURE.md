# Architecture

This document describes how Mosaic is put together: what each part of the code
base is for, how the parts talk to each other, and how the finished product is
meant to behave. It is a map, not a record of decisions. The reasoning behind
each choice lives in [`docs/adr/`](docs/adr/), the vocabulary in
[`CONTEXT.md`](CONTEXT.md), and the phased plan with its gates in
[`docs/plan.md`](docs/plan.md).

Throughout, **built** means the code exists and has been run against a real
runtime bundle, and **intended** means it is the design the code is heading
towards. [`docs/remaining-work.md`](docs/remaining-work.md) has the current
position between the two.

## The idea in one picture

Mosaic runs an Android app as an ordinary Linux process. No Android OS boots, no
container is created, and no kernel binder driver is needed. It reuses Android
where Android is portable, and replaces it where Android assumes it is running
on a phone.

```
             REUSED, UNMODIFIED                         REPLACED BY MOSAIC
  ┌──────────────────────────────────────┐   ┌──────────────────────────────────────┐
  │  the app (APK: DEX + resources)      │   │                                      │
  ├──────────────────────────────────────┤   │                                      │
  │  Java framework                      │   │  kernel binder  → userspace binder   │
  │   framework.jar, services.jar        │   │                   (shim + broker)    │
  │   ActivityThread, SystemServer,      │   │  init           → broker + property  │
  │   ActivityManagerService, ...        │   │                   service            │
  ├──────────────────────────────────────┤   │  zygote         → broker spawns apps │
  │  libandroid_runtime, libbinder, ...  │   │  system_server  → broker runs the    │
  ├──────────────────────────────────────┤   │                   real services      │
  │  ART (runtime, JIT, GC)              │   │  installd, vold,→ device services    │
  ├──────────────────────────────────────┤   │  netd, health...  hosted in broker   │
  │  Bionic libc + Android linker        │   │  SurfaceFlinger → per-app Wayland    │
  └──────────────────────────────────────┘   │                   toplevels          │
                    │                        │  AudioFlinger   → PipeWire           │
                    ▼                        │  SELinux        → real UIDs +        │
  ┌──────────────────────────────────────┐   │                   Landlock/seccomp   │
  │         host Linux kernel            │   └──────────────────────────────────────┘
  └──────────────────────────────────────┘
```

The left column is the Android **platform** (ADR-0002, ADR-0015). The right
column is the Android **device**. Mosaic's job is the right column.

## Processes

There are three kinds of process, and they are deliberately different binaries
with different C libraries (ADR-0009): glibc and Bionic cannot share a process,
so they talk over sockets.

```
 user's session                        systemd --user
 ──────────────                        ──────────────
 ┌──────────────┐  control frames     ┌─────────────────────────────────────────┐
 │ mosaic CLI   │────(JSON, socket)──▶│ broker   (mosaic daemon)                │
 │ (glibc, Rust)│                     │  glibc, Rust, unprivileged              │
 └──────┬───────┘                     │                                         │
        │ pkexec / sudo -n            │  • package registry (registry.json)     │
        ▼                             │  • install / launch / query             │
 ┌──────────────┐                     │  • binder authority: handle tables,     │
 │ uid-helper   │                     │    service names, refcounts, death      │
 │ (same binary,│                     │  • device services (installd, vold,     │
 │  root, one   │                     │    netd, health, SurfaceFlinger, ...)   │
 │  job)        │                     └───────────────▲─────────────────────────┘
 └──────────────┘                                     │ binder frames ("MSBD",
                                                      │ SCM_RIGHTS for fds)
                         ┌────────────────────────────┼───────────────────────┐
                         │                            │                       │
              ┌──────────┴──────────┐    ┌────────────┴────────┐   ┌──────────┴─────────┐
              │ framework process   │    │ app process         │   │ app process        │
              │ SystemServer on ART │    │ ActivityThread + app│   │ ...                │
              │ Bionic, shim preload│    │ Bionic, shim preload│   │ own system UID     │
              └─────────────────────┘    └─────────────────────┘   └────────────────────┘
```

| Process | Binary | Privilege | Lifetime |
| --- | --- | --- | --- |
| CLI | `mosaic <verb>` | the user | one command |
| Broker | `mosaic daemon` | the user, sandboxed by systemd | socket-activated, exits after 60 s idle |
| UID helper | `mosaic uid-helper` | root, via polkit | one allocation or removal |
| Framework / app processes | Bionic binaries from the runtime bundle | the app's own UID (intended) | while the app runs |

The broker is **not** a System Server and there is no boot sequence to wait for.
It is closer to Wine's `wineserver`: a rendezvous point that exists only while
something needs it (ADR-0005, ADR-0011).

## Source layout

```
src/
  main.rs            CLI entry: parse, log, dispatch a verb to the broker
  args.rs            clap command line (install, uninstall, launch, query,
                     runtime, daemon, uid-helper)
  lib.rs             module roots; the crate is also a library for tests
  config/            defaults (work directory), mosaic.cfg loading
  helpers/           http download + sha256 cache, logging, process runner
  actions/
    runtime_cmd.rs   `mosaic runtime fetch|status|verify|install`
    uid_helper.rs    the privileged helper and the code that invokes it
  runtime/           locate, fetch, unpack and run the runtime bundle
  broker/
    mod.rs           socket listener, systemd activation, idle exit, the
                     control-plane request handlers, the client side
    protocol.rs      control frames: 4-byte length + JSON Request/Response
    registry.rs      installed packages, atomic writes, name resolution
  binder/            userspace Binder, broker side
    wire.rs          data-plane frames ("MSBD" header, fds via SCM_RIGHTS)
    table.rs         one handle table per process, refcounts
    broker.rs        the authority: nodes, owners, routing, death notices
    transport.rs     one thread per connection; forwards and relays calls
    parcel.rs        writes/reads libbinder parcels for hosted services
    mod.rs           BinderObject trait, Answer, ServiceRegistry
  device/            Android device daemons, answered in userspace
    installd, apex, vold, storaged, netd, dnsresolver, health, suspend,
    idmap, surfaceflinger, display

tools/
  bundle/            build a runtime bundle from a system image (debugfs only);
                     property area generator; property service; logd
  binder-shim/       Bionic preloads that run *inside* Android processes:
                     android-binder.so, probe.so, android-properties.so,
                     pretend-nice.so, pretend-cgroups.so, alloc-trace.so
  launcher/          launcher.so: registers the framework's JNI natives and
                     runs a Java main class (e.g. SystemServer)
  boot-system-server.sh   the system of record for "how far does boot get"
  verify-*.sh        repeatable gates

systemd/             mosaic-broker.socket + .service (user units)
polkit/              the one authorised action: allocate a UID
data/, debian/, packaging/   desktop integration and distro packaging
docs/                ADRs, plan, binder and bundle records, remaining work
```

The Rust crate is the glibc side: CLI, broker, helper. Everything that has to
run inside an Android process is C in `tools/`, built with
`clang --target=x86_64-linux-android21 -nostdlib` so no NDK or AOSP tree is
required (ADR-0014).

## The runtime bundle

Every Android process is built from the **runtime bundle** (ADR-0010,
ADR-0012): a directory extracted from an unmodified Android system image that
contains the Android linker, Bionic, ART, `libandroid_runtime`, the framework
jars, ICU data and a generated linker configuration. It is not an OS image:
nothing in it boots.

```
 system.img ──(tools/bundle/bundle.sh build, debugfs, no root)──▶ bundle/
                                                                   ├─ linker64, lib64/…
                                                                   ├─ framework/*.jar
                                                                   ├─ apex/…
                                                                   ├─ ld.config.txt
                                                                   ├─ properties/   (prop area)
                                                                   ├─ bootclasspath.txt
                                                                   └─ env.sh, run.sh
```

`src/runtime/` finds a bundle (the user's under the work directory first, then
the machine-wide `/var/lib/mosaic/runtime`), downloads a published one with a
pinned sha256 (`mosaic runtime fetch`), or builds one from an image
(`mosaic runtime install <image>`). `bundle_env` sets the variables `init` would
have set on a device (`ANDROID_ROOT`, `ANDROID_DATA`, `ANDROID_ART_ROOT`, …) and
`run_in_bundle` is the single place a Bionic process is spawned from Rust.

A Bionic process starts like this:

```
 kernel ─▶ linker64 (from the bundle)
             │  reads ld.config.txt (one namespace, default visible)
             │  loads LD_PRELOAD / MOSAIC_PRELOAD: launcher.so, shim .so files
             ▼
           libc.so (Bionic) ─▶ libart.so ─▶ Java main (SystemServer / ActivityThread)
```

## Userspace Binder

Binder is the backbone of Android: every framework call between processes is a
Binder transaction. Mosaic implements it in two halves (ADR-0004, ADR-0014).

**Inside the Android process** a preloaded shim answers the calls libbinder
makes to the driver: `open("/dev/binder")`, `BINDER_VERSION`, the `mmap`, and
`BINDER_WRITE_READ` with its command stream (see [`docs/binder.md`](docs/binder.md)).
Because the shim shares the caller's address space, the pointers inside
`binder_transaction_data` can be read in place. Parcels stay in their native
format; the shim does not reinterpret them.

**In the broker** `src/binder/` plays the kernel driver's bookkeeping role:

- `table.rs`: every process has its own handle table, so one object has a
  different number in each process that can reach it. Handle 0 is always the
  service manager.
- `broker.rs`: every node has an owner. A process's own nodes are
  `pid << 32 | counter`; nodes the broker hosts start at `HOSTED_NODE_BASE`
  (`1 << 40`) so they cannot collide. It decides where a transaction goes:
  - `Dispatch::Reply`: a service the broker hosts answered it.
  - `Dispatch::Local`: the caller owns the target and runs it itself.
  - `Dispatch::Forward`: another process owns it.
- `transport.rs`: one thread per connection. A forwarded call is sent to the
  owner as `Incoming`, the owner answers with `IncomingReply`, and the broker
  relays it back as `Reply`, rewriting any handles and fds for the receiver.
  A synchronous call blocks its thread, as it does on a device, with a 30 s
  timeout so a dead owner cannot hold a caller forever.
- `wire.rs`: fixed 32-byte big-endian header starting with `MSBD`, then the
  body, with descriptors passed by `SCM_RIGHTS`. The broker tells a binder
  connection from a control connection by its first four bytes.

A cross-process call, end to end:

```
 app process                    broker                         framework process
 ───────────                    ──────                         ─────────────────
 BpBinder::transact
   └ ioctl(BINDER_WRITE_READ)
       └ shim: BC_TRANSACTION
           ── Transaction{h=7} ─▶ table[app][7] → node N
                                 owner(N) = framework
                                 rewrite object args into
                                 framework's handles
                                 ── Incoming{node N} ──────────▶ shim: BR_TRANSACTION
                                                                 BBinder::onTransact
                                 ◀── IncomingReply ──────────── BC_REPLY
                                 rewrite object results into
                                 app's handles, pass fds
           ◀── Reply ─────────── 
       └ shim: BR_REPLY
   └ Parcel back to Java
```

Reference counting (`Acquire`, `Release`, `IncRefs`, `DecRefs`), death
notification (`LinkToDeath`, `Dead`), name publication (`Export`) and lookup
(`Lookup`/`Found`) travel as their own message kinds on the same connection.

## Device services

The Java framework looks up a set of native daemons by name and blocks or aborts
if they are absent. Examples are `installd`, `vold`, `netd`, the health HAL,
`SystemSuspend`, `SurfaceFlinger` and the display HAL. `src/device/` implements each one
as a `BinderObject` and `device::host_all` publishes them all when the broker
starts, before any framework process can ask.

```rust
pub trait BinderObject: Send {
    fn transact(&mut self, code: u32, data: &[u8]) -> anyhow::Result<Answer>;
    fn transact_with(&mut self, code: u32, data: &[u8], args: &[u32]) -> ... ;
    fn descriptor(&self) -> &str;
}
```

An `Answer` carries the reply parcel, plus any objects handed back (a wake lock,
a display token), descriptors (a `BitTube` for display events), and calls to
make afterwards (a callback the caller registered). `ServiceRegistry` answers the
two protocol codes every object must answer: `PING_TRANSACTION` and
`INTERFACE_TRANSACTION`.

A desktop is not a device, so each service *answers* rather than *does*: the
health service reports the host's real battery, `installd` manages directories
under the bundle and the app's home, `netd` accepts that the host's network is
the network. Each file states what its service answers truthfully and what it
refuses.

## Properties

Bionic reads system properties from a shared-memory area at
`/dev/__properties__` and writes them over `/dev/socket/property_service`
(ADR-0013).

- **Reads:** `tools/bundle/make-property-area.py` writes the `property_info`
  trie and `prop_area` from AOSP's formats into `<bundle>/properties`. The
  values come from the image's `build.prop`, plus `dalvik.vm.*` values Mosaic
  chooses. Provisioning the fixed path is the one step that needs root.
- **Writes:** `tools/bundle/android-property-service.py` speaks bionic's
  128-byte `prop_msg` protocol, and `android-properties.so` interposes the write
  entry point so writes libc would refuse still go through.

The intended home of the property service is the broker, alongside the other
framework-facing services.

## Install, launch and isolation

Each app gets a real system UID and a data directory only it can read (ADR-0007).
Creating a user needs root and the broker is never root, so install is a
two-step transaction (ADR-0008):

```
 mosaic install app.apk
   │
   ├─1─▶ broker: Request::Install{apk}
   │     ◀── Response::Planned{name, uid, data_dir}   (reserved, not yet installed)
   │
   ├─2─▶ pkexec mosaic uid-helper <pkg> --uid N --apk …   (polkit prompt in the
   │       creates mosaic-<pkg> user, data dir, chown        user's own session)
   │
   └─3─▶ broker: Request::Commit{package}
         ◀── Response::Installed                        (written to registry.json)
```

A refused prompt leaves only a reservation, which is invisible, so retrying is
the same as starting again. Uninstall is the reverse: the broker removes the
registry entry and the CLI asks the helper to delete the user and data.

The registry (`src/broker/registry.rs`) is the only record of what is installed.
The broker is its only writer, and it saves through a temporary file and a
rename. `resolve` accepts an exact name, a unique prefix or a unique substring,
because nobody types the full package name.

**Launch (intended, P6).** `mosaic launch <pkg>` asks the broker, which spawns
the app process from the bundle under the app's UID with the shim preloaded.
The app process runs the real `ActivityThread`, which binds to the
`ActivityManagerService` inside the framework process through the broker. Today
`binder::launch_app` reports that this step is not implemented yet instead of
pretending to succeed.

The broker owns each app's state, as an explicit state machine rather than a
status string read from somewhere else (see `CONTEXT.md`):

```
 Installed ──launch──▶ Starting ──▶ Running ──exit──▶ Exited
```

## Windowing, input and audio (intended)

There is no compositor inside Mosaic (ADR-0006). Each app process gets its own
Wayland `xdg_toplevel`, and the host compositor composites everything.

```
 app process
   ViewRootImpl ─▶ libhwui (Skia) ─▶ ANativeWindow ──▶ Wayland surface (EGL/dma-buf)
                                                           │
                                            host compositor (Hyprland, GNOME, …)
                                                           │
   InputManager (Java) ◀── Android input events ◀── Wayland input
```

The broker's `SurfaceFlinger` and `display` services exist so that
`DisplayManagerService` can learn which displays exist and what modes they have.
They never touch pixels. Audio goes to PipeWire, and notifications and clipboard
go to the desktop's own services (P7). ARM-only native libraries are handled by
plugging FEX-Emu or box64 in as ART's native bridge (ADR-0003, P8).

## Configuration and deployment

| Piece | Where | Why |
| --- | --- | --- |
| Work directory | `~/.local/share/mosaic` (or `-w`) | registry, logs, user's bundle |
| Broker socket | `$XDG_RUNTIME_DIR/mosaic/broker.sock` (`MOSAIC_SOCKET` overrides) | user-reachable, `0600` |
| `mosaic-broker.socket` / `.service` | systemd user units | socket activation, idle exit, `ProtectSystem=strict`, `NoNewPrivileges` |
| `LimitNICE=40` + `user@.service.d` drop-in | system side | the framework raises thread priorities, and the limit can only be granted by the user manager |
| `id.mosaic.allocate-uid.policy` | polkit | the only privileged action |
| tmpfiles entry | system side | paths Bionic hardcodes (`/dev/__properties__`) |
| Shared bundle | `/var/lib/mosaic/runtime` | one read-only copy per machine |

Environment variables used on the Android side: `MOSAIC_PRELOAD` (the shim list),
`MOSAIC_BINDER_BROKER` / `MOSAIC_BINDER_SOCKET` (point the shim at the broker),
`MOSAIC_ANDROID_ROOT` (the bundle the framework runs from, which the broker's
device services must also use), `MOSAIC_PROPERTY_DIR`, and the traces
`MOSAIC_BINDER_TRACE` and `MOSAIC_ALLOC_TRACE`.

## How progress is measured

Every missing piece shows up when the code is run: an `UnsatisfiedLinkError`, a
service that is `not found; trying again`, a null where a handle should be. So
the method is a ratchet. Run `tools/boot-system-server.sh`, read the first wall,
fix it, and run again. The metric is how many `SystemServerTiming` boot stages
the real `com.android.server.SystemServer` reaches, and that number only goes up.
`tools/verify-a.sh` turns the current position into a repeatable gate, and it
treats a shim or launcher build warning as a failure.

```
 P1 bundle ✅ ─▶ P2 app process + properties ✅ ─▶ P3 userspace binder ─▶ P4 SystemServer
   ─▶ P5 Wayland windows ─▶ P6 real install/launch ─▶ P7 input, audio ─▶ P8 ARM
   ─▶ P9 media, long tail ─▶ P10 packaging and release
```

## Security model

What Mosaic provides: a separate real UID per app, `NoNewPrivileges`, an
unprivileged broker (so an IPC routing bug is never a root bug), a single-purpose
root helper, and, as intended, Landlock and seccomp on each app process. What it
does not provide is Android's SELinux policy. A malicious app is stopped from
reading another app's data, but not from everything SELinux would catch. The plan
states this gap openly ([`docs/plan.md`](docs/plan.md#cross-cutting-the-security-model-honestly)),
and so does this document.

Tier 4 apps are permanently out of scope: anything that needs Google Play
services, hardware attestation or DRM.
