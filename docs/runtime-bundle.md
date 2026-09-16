# The runtime bundle: what it is, and what is proven

The runtime bundle is the pinned archive of host-native Bionic and ART that
every app process runs on top of (ADR-0010). This document records what the
bundle turned out to contain, with the commands and outputs that established
it, so the next step starts from evidence instead of from the design sketches.

Everything below was done with the system image the retired container port had
already downloaded, `/var/lib/mosaic/images/system.img`, using only `debugfs`
(no root, no loop mount, no container). The tooling is
[`tools/bundle/bundle.sh`](../tools/bundle/bundle.sh).

## A bundle is a directory of libraries, not an image

| Piece | Where it came from |
| --- | --- |
| `linker64` | `/system/apex/com.android.runtime/bin/linker64` |
| Bionic (`libc.so`, `libm.so`, `libdl.so`, `libdl_android.so`) | `/system/apex/com.android.runtime/lib64/bionic/` |
| ART (`libart.so`, `libartbase.so`, `libdexfile.so`, …) | `/system/apex/com.android.art/lib64/` |
| Support libraries | `/system/lib64/` and the other apexes |
| Boot classpath jars (`core-oj.jar`, `core-libart.jar`) | `/system/apex/com.android.art/javalib/` |
| Linker configuration | generated, see below |

Two traps cost the most time and are worth stating plainly:

- `/system/lib64` is mostly symlinks into `/system/apex/*`. Dumping a symlink
  produces a zero-byte file, and the linker reports it as
  `file offset for the library "libc.so" >= file size: 0 >= 0`.
- A `dlopen` failure can be completely silent. Walking `DT_NEEDED` with
  `readelf` finds those gaps before the linker does. `libart.so` is `dlopen`ed
  and needs 20-odd libraries of its own; the linker named none of them.

## Proven: a Bionic program runs on the host kernel

```
$ tools/bundle/bundle.sh run <bundle> toybox echo "hello from Bionic"
hello from Bionic
```

`toybox` was staged with 13 shared libraries extracted from the image plus a
seven-line linker config. No container, no namespaces, no kernel module, no
Android OS. `uname -a` and `toybox sh` work too.

## Proven: ART initializes

```
$ tools/bundle/bundle.sh run <bundle> dalvikvm64 -XXlib:<bundle>/lib64/libart.so -showversion
ART version 2.1.0 x86_64
```

The full `dalvikvm64 -help` text prints as well. The `-XXlib:` argument is
needed because `dalvikvm` `dlopen`s `libart.so` rather than declaring it as a
dependency, and it looks for it by a path the generated linker config would
have provided on a device.

## Blocked: executing a DEX

```
$ tools/bundle/bundle.sh run <bundle> dalvikvm64-patched \
    -XXlib:<bundle>/lib64/libart.so \
    -Xbootclasspath:<bundle>/javalib/core-oj.jar:<bundle>/javalib/core-libart.jar \
    -cp <bundle>/framework/am.jar com.android.commands.am.Am
Failed to get system namespace for loading libandroid.so
libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 154651 (main), pid 154651 (main)
```

The message comes from `libnativebridge.so` (and `libart.so` has a sibling for
OAT files). ART and the native bridge ask the linker for the exported `system`
namespace by name, and the hand-written linker config does not produce one.
The equivalent config Android ships is generated at boot by `linkerconfig` from
`/system/etc/linker.config.pb` and the system properties; the image contains the
input, not the output, and the retired port's container never got far enough to
generate one.

Two things were ruled out along the way, since both are plausible and neither
was the cause: the interpreter (`PT_INTERP`) naming `/system/bin/linker64`, and
the linker being run as a program rather than by the kernel. Patching the
interpreter with `patchelf --set-interpreter <bundle>/linker64` is still the
right way to launch a binary, and the failure is identical either way.

## Next step

Produce a correct linker config. In order of preference:

1. Run `linkerconfig` from the image against a bundle layout, and ship its
   output. It is a static Android binary in the runtime apex.
2. Model the generated file directly, checking each namespace against what
   `libnativebridge.so` and `libart.so` look up by name.
3. Build Bionic's linker with the bundle layout as its default, since
   ADR-0002 commits the project to building Bionic anyway. This removes the
   whole class of problem rather than reproducing Android's boot-time
   configuration generation.

Whichever it is, the gate is unchanged: a DEX running on ART, with no Android
OS booted and no kernel module loaded.
