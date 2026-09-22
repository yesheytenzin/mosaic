<img align="left" src="data/AppIcon.png" width="64">

# Mosaic

Mosaic runs Android apps as native Linux processes. There is no Android OS to
boot, no container to manage, and no kernel binder driver to install. An
installed app is an ordinary desktop application: it opens a real Wayland
window, and its data lives under its own system user.

Installing an app allocates it a protected system user, the same isolation model
Android itself uses, so one app cannot read another's files. The Android API
surface, framework services, Binder and the hardware abstraction layer are
translated to host equivalents, with AOSP's ART and Bionic reused as host-native
libraries rather than reimplemented.

Mosaic began as a Rust port of Waydroid's container model. That port is being
retired, for the reasons in `docs/adr/0001`. Containers boot an entire Android
system to run one app and depend on a kernel binder driver many hosts do not
ship.

## Status

Design and scaffolding, with the first executable milestone met and well past it.
The architecture is recorded in `CONTEXT.md` and `docs/adr/`. A single DEX runs on
a host-native ART and Bionic build with no Android OS; `com.android.server.SystemServer`
boots its real services through `startBootstrapServices` to `StartDisplayManager`;
a userspace Binder carries a transaction between two processes, including one that
runs the framework. What is left, in dependency order, is
[`docs/remaining-work.md`](docs/remaining-work.md).

## Build

Requires a Rust toolchain, 1.74 or newer.

```
cargo test
make build
```

## Runtime

Mosaic runs on an Android runtime bundle: the system image's ART, Bionic and
framework jars, unmodified. A published one for x86_64 installs with one command,
no image, no checkout and no build tools:

```
mosaic runtime fetch
```

`mosaic runtime install <image>` builds one from a system image instead, and
`mosaic runtime install <url>` fetches one from anywhere -- a mirror, a release, a
directory on a local network. [`docs/runtime-bundle.md`](docs/runtime-bundle.md)
records what the bundle contains and
[`docs/remaining-work.md`](docs/remaining-work.md) how it is published.

## Documentation

Start with [`CONTEXT.md`](CONTEXT.md) for the vocabulary and
[`docs/adr/`](docs/adr/) for the decisions and their reasoning.
[`docs/plan.md`](docs/plan.md) is the plan for running any Android app, with the
phases, their gates, and the risks that could change it.
[`docs/runtime-bundle.md`](docs/runtime-bundle.md) records what the runtime
bundle actually contains and what is proven so far, with the tooling in
[`tools/bundle/`](tools/bundle/). [`docs/binder.md`](docs/binder.md) records what
the binder driver interface actually has to answer, and
[`tools/binder-shim/`](tools/binder-shim/) is the shim that found out.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
