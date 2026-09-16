# Bionic is the app process libc, not a shim over glibc

An Android app process expects Bionic. Bionic and glibc cannot coexist in one
process: they define the same symbols with different ABIs and semantics, so
linking both is not a matter of care but of impossibility. The alternative, a
Bionic-ABI shim implemented over glibc, would mean reimplementing the libc
surface that apps and native libraries actually touch, which is orders of
magnitude more work than reusing the real thing and contradicts ADR-0002.

Mosaic therefore builds ART and Bionic together as an x86_64 Android-userland
pair, and each app process links Bionic as its libc. It runs directly on the
host kernel, since Android binaries run on a Linux kernel. Only the broker and
the privileged helper are glibc Rust binaries. They never share a process with
app code; they speak to app processes over IPC. This is the same partition Wine
uses: one host process, one guest-ABI process, a protocol between them.
