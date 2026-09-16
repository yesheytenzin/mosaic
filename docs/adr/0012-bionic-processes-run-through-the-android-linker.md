# A Bionic process is loaded by the Android linker, from a bundle-local config

ADR-0002 says Mosaic reuses ART and Bionic, and ADR-0009 says Bionic is the
app process's libc. Neither said how such a process actually starts, and the
obvious answer is wrong: AOSP's *host* ART build links glibc, so the host build
cannot be the app process. The app process has to be an Android-target process
that the kernel loads directly, with Bionic as its libc.

Mosaic launches each app process through the Android dynamic linker from the
runtime bundle. The kernel loads the linker (either because the binary's
`PT_INTERP` names it, or because the linker is run with the program as an
argument), the linker loads Bionic and the ART runtime out of the bundle, and
the process runs as an ordinary host process. Three things come with that and
are part of the bundle, not part of the host:

- the linker configuration Android generates at boot with `linkerconfig` (the
  generated `/linkerconfig/ld.config.txt`), rewritten to point at the bundle.
  The linker consults it for every namespace decision, including the exported
  `system` namespace that ART and `libnativebridge` ask for by name.
- the environment `init` would have set: `ANDROID_ROOT`, `ANDROID_DATA`,
  `ANDROID_ART_ROOT`, `ANDROID_I18N_ROOT`, `ANDROID_TZDATA_ROOT`, `ANDROID_TMP`.
  Without them ART cannot find its configuration or a place to write
  `dalvik-cache`.
- a writable data directory for `dalvik-cache` and the compiled artifact cache.

`PT_INTERP` is an absolute path, so the bundle lives at a fixed location the
packaging step chooses. That is the price of not creating the absolute paths an
Android image uses, which would mean a mount namespace and a return toward the
container model this project exists to avoid.

Verified so far: a Bionic `toybox` runs, and ART initializes far enough to print
`ART version 2.1.0 x86_64` and its full option list. Executing a DEX stops at
`Failed to get system namespace for loading libandroid.so`, which is exactly the
linker-config gap above. See `docs/runtime-bundle.md` for the record.
