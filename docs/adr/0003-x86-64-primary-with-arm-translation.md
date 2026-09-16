# x86_64 as the primary target, with ARM to x86 translation

Android APKs commonly ship prebuilt ARM `.so` native libraries, but Waydroid's
users are largely on real ARM hardware where no translation is needed. Mosaic
targets x86_64 desktop Linux as the primary platform and integrates an existing
ARM-to-x86 translator (FEX-Emu or box64) for prebuilt native code, rather than
staying ARM64-only or writing a translator. Pure Java and Kotlin apps need no
translation at all once ART is built for x86_64; the translation problem is
scoped to prebuilt native `.so` code.
