# Reuse ART and Bionic rather than reimplementing them

Full Android app compatibility requires executing DEX bytecode and linking
against Android's C library ABI. Mosaic builds AOSP's ART runtime and Bionic libc
as host-native x86_64 libraries and links them into each app process, rather
than writing a new bytecode VM or libc. Only the framework-services, Binder and
HAL layer around them is new project code. ART is a many-person-decade JIT, GC
and verifier project; reimplementing it is not where native execution adds
value. This mirrors Wine reusing the real x86 CPU instead of reinventing code
execution.
