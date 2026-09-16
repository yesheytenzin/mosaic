# Runtime bundle tooling

`bundle.sh` builds and inspects a Mosaic runtime bundle straight out of an
Android system image (ADR-0010, ADR-0012). It needs only `debugfs`, `readelf`,
and optionally `patchelf`: no root, no loop mounts, no container.

```
bundle.sh index   <image> <bundle>                  # catalog the image's libraries
bundle.sh stage   <image> <bundle> <inode> <name>   # extract one binary and chase its libraries
bundle.sh closure <image> <bundle>                  # fill in every missing dependency
bundle.sh run     <bundle> <binary> [args...]       # run it with the environment a Bionic process expects
```

`stage` is the entry point when starting from nothing. It extracts the binary,
points its interpreter at the bundle's linker, then runs it and reads the
missing library out of the linker's error until the program starts. `closure`
is the maintenance path: it walks `DT_NEEDED` for everything already staged and
reports what is missing before the linker does.

## Seed list

A library loaded with `dlopen` never appears in `DT_NEEDED`, so nothing can
infer it. Put those names in `<bundle>/seed.txt`, one per line:

```
libartpalette-system.so
libjavacore.so
libopenjdk.so
```

Everything else is discovered.

## Why the image is read with debugfs

`debugfs` reads an ext4 image in place, so no loop device and no root. Two
details matter:

- The listings use `ls -l`. `debugfs ls` without `-l` prints a compact form that
  silently drops entries, which produced an index missing half the apexes.
- `/system/lib64` is mostly symlinks into `/system/apex/*`, so the index records
  whether an entry is a real file and only real files are staged. A symlink
  dumped as a file is zero bytes, which the linker reports as
  `file offset for the library ... >= file size`.

## Linker configuration

`bundle.sh` writes a starting `ld.config.txt` if the bundle does not have one.
It is enough for a plain Bionic program such as `toybox`, but not for ART, which
additionally looks up the exported `system` namespace by name. See
[`docs/runtime-bundle.md`](../../docs/runtime-bundle.md) for where that stands.

The environment `bundle.sh run` sets (`ANDROID_ROOT`, `ANDROID_DATA`,
`ANDROID_ART_ROOT`, and the rest) is what `init` sets on a device. Without it
ART cannot find its configuration or a place to write `dalvik-cache`.
