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

The linker configuration is the part that is easy to get wrong, and the
consequence is worth stating in the decision rather than in a comment: the
namespace that holds libc.so must be the one ART and libnativebridge look up by
name. A separate namespace named `system` looks equivalent and is not, because
ART's classloader namespace then becomes a child of an empty namespace, so every
later `dlopen` loads a second copy of libc.so, which bionic refuses since libc
uses initial-exec TLS. The bundle therefore declares one section covering the
whole bundle with its default namespace visible.

Verified: a Bionic `toybox` runs, ART prints `ART version 2.1.0 x86_64`, and a
DEX executes. `am.jar` from the image starts, initialises the framework classes,
and fails only where it needs a JNI method that `libandroid_runtime.so` would
register — which is Phase 2 work, not a problem with this decision. See
`docs/runtime-bundle.md` for the record and `tools/bundle/` for the tooling.
