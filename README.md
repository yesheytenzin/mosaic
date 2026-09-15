<img align="left" src="data/AppIcon.png" width="64">

# Mosaic (Rust)

A Rust rewrite of [Waydroid](https://github.com/waydroid/waydroid). Mosaic
boots a full Android system in an LXC container using Linux namespaces (user,
pid, uts, net, mount, ipc) and runs Android applications on a regular
GNU/Linux system.

The Android runtime ships a minimal LineageOS-based image, currently on
Android 13.

## Build

Requires a Rust toolchain (1.74 or newer) and a C toolchain for the XZ and TLS
dependencies.

```
make build
```

## Install

```
sudo make install
```

This installs the `mosaic` binary to `/usr/bin`, data files to
`/usr/lib/mosaic`, plus the D-Bus, systemd, and polkit units. AppArmor
profiles are installed separately:

```
sudo make install_apparmor
```

## Usage

```
mosaic init                    # set up configs and download images
mosaic session start           # start a user session
mosaic app launch <package>    # launch an installed app
mosaic status                  # show session state
```

Run `mosaic --help` for the full command list. Runtime dependencies are
`lxc`, `dbus`, a polkit authority, a PulseAudio or PipeWire server, and
`iptables`.

## Documentation

Upstream documentation is at [docs.waydro.id](https://docs.waydro.id).

## Reporting bugs

File issues at <https://github.com/yesheytenzin/mosaic/issues>.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
