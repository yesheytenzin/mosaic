# Binder without a kernel driver

ADR-0004 says Mosaic replaces the kernel Binder driver with a userspace
implementation. This is what that actually requires, established by preloading a
logging shared object into `app_process64` rather than by reading the driver's
source. The tool is [`tools/binder-shim/`](../tools/binder-shim/); the record
below is its output.

## The device surface

`libbinder` needs exactly this from `/dev/binder`, in this order, and nothing
else so far:

| Call | Answer |
| --- | --- |
| `open("/dev/binder", O_RDWR \| O_CLOEXEC)` | any fd that can be `mmap`ed |
| `ioctl(BINDER_VERSION, &v)` | `v = 8` (`BINDER_CURRENT_PROTOCOL_VERSION`) |
| `ioctl(BINDER_SET_MAX_THREADS, &n)` | accept |
| `ioctl(BINDER_ENABLE_ONEWAY_SPAM_DETECTION, &n)` | accept; advisory |
| `mmap(NULL, 1040384, ...)` | `BINDER_VM_SIZE`, one mapping |
| `ioctl(BINDER_WRITE_READ, &bwr)` | the transactions |

That is the whole interface. There is no separate read or write path, no
`ioctl` for the transaction buffer beyond `BINDER_WRITE_READ`, and the mapping is
opened once.

`BINDER_WRITE_READ` carries a command stream in `write_buffer` and expects a
command stream back in `read_buffer`. The commands come with fixed operands and
no padding: a transaction is the command word followed by a 64-byte
`binder_transaction_data`, a handle command is the word plus a `u32`, and
`BC_FREE_BUFFER` is the word plus a pointer.

After startup the first real traffic is transactions to **handle 0**, which is
the context manager:

```
BINDER_WRITE_READ write_size=76  commands: BC_TRANSACTION(handle=0 code=16 ...)
```

`handle 0` is the service manager, so the first userspace service to implement is
the service registry, which is also what ADR-0005 gives the broker.

## Why a preloaded shim

The alternative was to present a real character device with CUSE. That needs
`/dev/cuse`, which is root-only, and it means writing a kernel ABI in userspace
twice over: once for CUSE and once for Binder.

A preloaded library is less code and needs no privilege, and the shim runs
*inside* the app process, which has a useful consequence: the buffer pointers in
`binder_transaction_data` are valid in the shim's own address space. A driver in
another process would have to copy them across a boundary. Here they can be read
where they are.

The app process side is a Bionic shared object built with plain `clang`:

```
clang --target=x86_64-linux-android21 -shared -fPIC -nostdlib ...
```

No NDK is needed, and `-nostdlib` is correct rather than a compromise: a shared
object may leave its libc symbols undefined, and the Android linker resolves them
when it loads the library. See `tools/binder-shim/build.sh`.

## Two things that are easy to get wrong

**Bionic's `<fcntl.h>` makes `open()` an inline wrapper around `__openat`, and
`_FORTIFY_SOURCE` makes it `__open_2`.** A preloaded `open()` is never called.
The shim had to intercept `__open_2` before it saw anything, and finding that
took a probe that interposed a function known to be called (`__system_property_find`)
to prove interposition worked at all.

**Symbol versioning is in the way of nothing, but it looks like it is.**
`libbinder` imports `open@LIBC`, `ioctl@LIBC`, and `mmap@LIBC`; an unversioned
definition in a preloaded library still wins.

## Where it stands

The shim answers the version, threads, spam-detection and mapping calls, so
`ProcessState::self()` succeeds and the framework gets a real `IBinder` for the
context manager. `BINDER_WRITE_READ` is parsed and logged but refused, so the
framework stops on the first transaction, which is exactly the call the service
manager has to answer.

What is left for Phase 3:

1. Reply to `BINDER_WRITE_READ`: consume the write stream, and for a transaction
   to handle 0 dispatch it to the service registry and write a `BR_REPLY` with
   the resulting handle.
2. Reference counting: `BC_ACQUIRE`/`BC_RELEASE`/`BC_INCREFS`/`BC_DECREFS`, which
   are what keep a remote object alive.
3. Route transactions to handles the registry handed out. For now those services
   do not exist, so the honest answer is an error reply rather than a hang, and
   the framework reports "service unavailable" instead of crashing.
