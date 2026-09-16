# Runtime bundle tooling

The tools that build a Mosaic runtime bundle out of an Android system image and
that run Bionic processes from it (ADR-0010, ADR-0012). They need only
`debugfs`, `readelf`, and optionally `patchelf`: no root, no loop mounts, no
container.

## bundle.sh

```
bundle.sh build   <image> <bundle>                  # produce a complete, runnable bundle
bundle.sh index   <image> <bundle>                  # catalog the image's libraries
bundle.sh closure <image> <bundle>                  # fill in every missing dependency
bundle.sh stage   <image> <bundle> <inode> <name>   # extract one binary and chase its libraries
bundle.sh jars    <image> <bundle>                  # boot classpath, ICU data, bootclasspath.txt
bundle.sh run     <bundle> <binary> [args...]       # run it with the environment a Bionic process expects
bundle.sh compile <bundle> <apk> [filter]           # dex2oat an app's bytecode
```

`compile` is the closest thing to running a real app that the runtime alone can
do, and it is a good end-to-end check of a bundle: it loads an app's DEX,
verifies it against the boot classpath, and emits machine code. It needs no root
and no namespace. Verified on Termux 0.118.3, which compiles to 9 MB of OAT with
14,225 methods.

Give it a small heap (`-Xmx256m`, which it sets): ART commits what `-Xmx` asks
for, so a 1 GB heap fails the compiler's arena mapping on a busy host while
plenty of memory is free.

`build` is the one that matters, and it is the input to every later phase: the
Android linker, the library closure, the boot classpath jars, ICU data, a linker
configuration, and `env.sh` / `run.sh`.

`stage` is the entry point when starting from nothing. It extracts a binary,
points its interpreter at the bundle's linker, then runs it and reads the
missing library out of the linker's error until the program starts. `closure`
is the maintenance path: it walks `DT_NEEDED` for everything already staged and
reports what is missing before the linker does.

## with-logd.sh

```
MOSAIC_PROPERTY_DIR=<dir> with-logd.sh <command> [args...]
```

Runs a Bionic process with the `/dev` entries it expects, which a host does not
have and which need root to create for real:

- a working **logd**. liblog sends to `/dev/socket/logdw` and drops messages when
  nothing is listening, so without this an Android process aborts silently.
- an optional **property area**. Point `MOSAIC_PROPERTY_DIR` at a directory built
  by `make-property-area.py` and it appears at `/dev/__properties__`. This is
  where libc reads system properties, hardcoded, and ART and `app_process` will
  not start without it.

It sets up a private `/dev` with `unshare -rm`, where the files it creates are
owned by uid 0 as libc requires. Output goes to stderr.

This is a debugging aid. The product runs app processes in the host's own
namespace (ADR-0001); it does not wrap them in one.

## make-property-area.py

```
make-property-area.py <dir> [--build-prop FILE] [--set NAME=VALUE ...]
```

Generates the Android property area: the `property_info` trie that maps a
property name to a context, the `properties_serial` area libc requires, and one
`prop_area` per context holding the values. Both formats are implemented from
AOSP (`prop_area.h`, `prop_info.h`, `property_info_parser.h` and their
implementations), because there is no library in the bundle that can generate
them: on a device `init` does, at boot.

`bundle.sh build` runs it into `<bundle>/properties`, seeded from the image's
`build.prop`, for the privileged provisioning step to install.

Values come from three places, in order of precedence: `--set`, the Mosaic
defaults in the script (notably `dalvik.vm.*`, which is the runtime's to choose),
and the image's `build.prop` where it defines a property Mosaic also defines.
Only those keys are taken; a whole `build.prop` does not fit in one area, and a
property with no value reads as unset anyway.


## Things that are easy to get wrong

Each of these cost more than an hour, and each is encoded in the tool.

- **`/system/lib64` is mostly symlinks into `/system/apex/*`.** A symlink dumped
  as a file is zero bytes, which the linker reports as
  `file offset for the library ... >= file size`. The index records whether an
  entry is a real file, and only real files are staged.
- **`debugfs ls` without `-l` prints a compact form that silently drops
  entries.** One index built that way was missing half the apexes. Every listing
  uses `-l` and takes the last field as the name.
- **A `dlopen` failure can name no library at all.** `libart.so` is `dlopen`ed
  and needs about twenty libraries of its own; the linker named none of them.
  Walking `DT_NEEDED` with `readelf` finds the gaps first.
- **Libraries loaded with `dlopen` are invisible to that walk.** They go in
  `<bundle>/seed.txt` by name. `build` also seeds the device's
  `public.libraries.txt`, which ART preloads every entry of.
- **The namespace holding libc.so has to be the one ART looks up by name.** See
  the comment in `write_linker_config`. A separate namespace named `system`
  looks right and makes every later `dlopen` try to load a second `libc.so`,
  which bionic refuses because libc uses initial-exec TLS.
- **Match basenames as strings, not regexes.** `libc++.so` as a regex matches
  `libc.so`, which staged libc as the C++ runtime and produced a symbol error
  two steps later.

## Where a bundle comes from

`build` reads an existing Android system image. The retired container port
already downloaded one to `/var/lib/mosaic/images/system.img`, and that is what
the record in [`docs/runtime-bundle.md`](../../docs/runtime-bundle.md) uses.
Publishing a pinned bundle as a release artifact is still to do (ADR-0010).
