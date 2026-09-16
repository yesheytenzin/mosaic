# Userspace Binder is reached by a preloaded shim, not a device

libbinder opens `/dev/binder` and speaks to it with a handful of ioctls. ADR-0004
commits Mosaic to implementing that interface in userspace rather than shipping a
kernel module. There are three ways to put a userspace implementation behind
those calls, and the choice matters:

- **CUSE.** Present a real character device at `/dev/binder`. This needs
  `/dev/cuse`, which is root-only, and it means implementing the driver ABI twice:
  once as a CUSE device and once as Binder.
- **A kernel module.** What Android does, and what ADR-0004 rules out.
- **A preloaded shared object.** Intercept the device calls inside the app
  process and serve them from a userspace binder.

Mosaic uses the preloaded shim, exposed as `MOSAIC_PRELOAD`. It needs no
privilege, it is far less code, and the app process side is a Bionic shared
object that plain `clang` can build with `-nostdlib`, so no NDK or AOSP tree is
required to produce it.

There is a structural benefit beyond the privilege and the code: the shim runs in
the same address space as the caller, so the buffer pointers inside
`binder_transaction_data` are directly readable. An implementation in a separate
process would have to copy each transaction across a boundary, and every pointer
the caller passes would need translating.

The transport behind the shim is a Unix socket to the broker, which owns the
service registry (ADR-0005). The shim is the client end of it, and the broker is
the process that stays alive between apps.

The consequence to accept is that the shim is a preloaded library, so a process
must be started with it. That is already true: the broker starts every app
process, and the bundle's wrapper applies `MOSAIC_PRELOAD` to the Bionic process
and not to the shell around it.
