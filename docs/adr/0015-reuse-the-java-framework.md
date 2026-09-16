# Reuse the Android Java framework, replace only the device layer

ADR-0002 decided that Mosaic reuses ART and Bionic rather than reimplementing
them. The next layer up needs the same decision, and it is the one the whole plan
rests on: the Android Java framework.

There are two options. Reimplement the framework — `ActivityThread`,
`ActivityManager`, `PackageManager`, the view hierarchy, the resources system —
or reuse AOSP's, which is already in the runtime bundle as `framework.jar` and
already executes: the framework's static initialisation runs, `Build.VERSION` and
`RuntimeInit` complete, and `am` reaches its first Binder call (see
`docs/runtime-bundle.md`).

Mosaic reuses the framework and replaces only the device layer. The framework is
the Android *platform*: it is portable, it is where the semantics apps depend on
live, and reimplementing it is the largest and least rewarding part of the
project. Binder's kernel driver, `init`, `zygote`, `system_server`'s process
wrapper, SurfaceFlinger, `installd` and `vold` are the Android *device*: they
exist to talk to Android hardware and to a kernel Android controls, and none of
them can be reused on a desktop. That is the same partition ADR-0002 drew, one
layer up.

Concretely this means the broker runs the real `com.android.server.*` classes —
`ActivityManagerService`, `PackageManagerService`, `WindowManagerService` — and
provides the native and device pieces they expect, and app processes run the real
`ActivityThread` against the same. The work is not reimplementation but a
sequence of gaps, each announced at runtime as an `UnsatisfiedLinkError` or a
missing device, which is what makes it measurable: the metric is how far into
`system_server`'s boot the process gets.

Two consequences follow, and both are load-bearing.

The replacement device layer has to be written as Bionic-compatible native code,
because the framework it serves is Bionic. That is possible without an NDK —
ADR-0014 established that `clang --target=x86_64-linux-android21` builds shared
objects the Android linker loads — but it is a real constraint on how that layer
is written.

And the plan acquires a kill criterion. If `SystemServer` cannot boot on a
shimmed platform because it needs an unbounded set of real-device semantics
rather than a list of natives, this decision is wrong and the fallback is the
much larger job of reimplementing the minimum service surface. The plan says to
test that before committing to anything downstream (the P4 spike).
