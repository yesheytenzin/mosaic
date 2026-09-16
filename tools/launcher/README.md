# launcher

Runs any class with the Android framework's JNI natives registered.

`dalvikvm` runs an arbitrary class from an arbitrary class path but never
registers the framework's natives, so `android.os.Binder` and friends report
"No implementation found". `app_process` registers them, through
`AndroidRuntime::start` → `startReg`, but `startReg` is not exported and
`app_process` is not a general launcher.

The registrars `startReg` calls *are* exported, though: the bundle's
`libandroid_runtime.so` exports 150 symbols named `register_*` (see
`registrars.inc`, generated at build time from whichever image the bundle came
from). This does what `startReg` does — calls each of them with a `JNIEnv` — and
then runs a `main`.

## Building

```
./build.sh <bundle> [output-dir]
```

Needs `clang`, `readelf`, and a bundle. No NDK: `clang --target=x86_64-linux-android21
-shared -nostdlib` produces a Bionic shared object whose libc symbols the Android
linker resolves at load time.

## Running

It is a shared object rather than an executable because a Bionic executable needs
the Android crt objects, which the image does not ship. It is preloaded into a
Bionic binary that creates a VM — `dalvikvm` will do — and interposes
`JNI_CreateJavaVM`, so the work happens when the host asks for the VM and the
launcher takes over before the host's own body runs.

A constructor would not work: it runs before libc has set up `environ`, so the
environment is unreadable. That was the first design and it read nothing.

```
MOSAIC_PRELOAD=<out>/launcher.so \
MOSAIC_LAUNCH_CLASS=com.android.server.SystemServer \
MOSAIC_LAUNCH_RUNTIME=$BUNDLE/lib64/libandroid_runtime.so \
bundle/run.sh dalvikvm64 -Xbootclasspath:"$(cat $BUNDLE/bootclasspath.txt)" \
  -cp "$BUNDLE/framework/services.jar:$BUNDLE/framework/framework.jar"
```

The class path and boot class path come from the host's own arguments, so the
invocation stays an ordinary `dalvikvm` one; only the class to run is ours.

## Registration order matters

`registrars.inc` is ordered by `registrar_order.txt`, which is AOSP's own order
from `AndroidRuntime.cpp`. Alphabetical order looks harmless and is not: it runs
`register_android_os_Binder` before `register_android_util_Log`, and Binder's
class initialisation touches `StrictMode`, which calls
`android.util.Log.isLoggable` — so registration aborts on a native that would have
been registered a few lines later. The message is worth recognising:

```
No implementation found for boolean android.util.Log.isLoggable(java.lang.String, int)
  at android.os.StrictMode.<clinit>(StrictMode.java:155)
```

## What it reached

With the framework's natives registered:

```
launcher: registered 150 native registrars
launcher: running com.android.server.SystemServer
SystemServerTiming: InitBeforeStartServices
```

and then it waits for the first Binder transaction, which is P3's work.
