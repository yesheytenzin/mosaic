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

Design and scaffolding. The architecture is recorded in `CONTEXT.md` and
`docs/adr/`. The first executable milestone is running a single DEX on a
host-native ART and Bionic build with no Android OS, which is the gate for
everything else.

## Build

Requires a Rust toolchain, 1.74 or newer.

```
cargo test
make build
```

## Documentation

Start with [`CONTEXT.md`](CONTEXT.md) for the vocabulary and
[`docs/adr/`](docs/adr/) for the decisions and their reasoning.
[`docs/runtime-bundle.md`](docs/runtime-bundle.md) records what the runtime
bundle actually contains and what is proven so far, with the tooling in
[`tools/bundle/`](tools/bundle/). [`docs/binder.md`](docs/binder.md) records what
the binder driver interface actually has to answer, and
[`tools/binder-shim/`](tools/binder-shim/) is the shim that found out.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
