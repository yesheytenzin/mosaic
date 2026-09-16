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

## The framing, as far as it goes

Two attempts to find the command stream in `write_buffer` have now failed, and
the failures are more informative than the guesses:

- Following every plausible pointer in the first 64 bytes, with a
  `/proc/self/maps` readability check so it cannot crash, finds **no pointer at
  all**.
- Searching the whole buffer for the transaction this call must be — `BC_TRANSACTION`,
  a null target (handle 0, the service manager) and an AIDL code in 1..8 — finds
  **nothing**.

So the problem is not the offset. `write_buffer` is a real heap address, the
`binder_write_read` fields read correctly (sizes 68/76 and 256, two plausible
pointers), and yet the bytes there are mostly zeros with a few odd values. That is
not a Parcel's data and not a command stream, which means the assumption to
question is `write_buffer == mOut.data()`. The next attempt should instrument the
*sender*: log `IPCThreadState::mOut`'s size and data pointer from inside
`IPCThreadState::talkWithDriver`, or read that function again with this evidence in
hand, rather than reading more bytes at the receiving end.

A segfault was caused along the way by following a pointer without checking it was
mapped, so the safety check is worth keeping in whatever diagnostic comes next.

## Where it stands

The shim answers the version, threads, spam-detection and mapping calls, so
`ProcessState::self()` succeeds and the framework gets a real `IBinder` for the
context manager. It parses `BINDER_WRITE_READ` and fails fast on a stream it does
not understand, which keeps a mistake in Java rather than in a spin.

The command stream's exact framing is **not yet settled**, and that is the next
thing to do. Every write buffer observed begins with bytes that are not a command
word:

```
write_size=68  [00 63 40 40 00 00 00 00 00 00 00 00 ... 47 4e 50 5f]
write_size=8   [05 63 04 40 00 00 00 00]
```

The first four bytes on the two calls differ, so it is not a fixed prefix, and the
three bytes after `00` on the first line read like the low bytes of a pointer into
a `0x40xxxxxx` mapping. The next step is to read the layout out of libbinder
rather than infer it from bytes: `IPCThreadState::talkWithDriver` and how it sets
`write_buffer` from its outgoing `Parcel`, and what `Parcel::data()` and
`dataSize()` count. `tools/binder-shim/probe.c` prints the first bytes of every
write buffer for exactly this.

An early attempt at answering the transaction produced a process that spun until
it filled a 5 GB log, because libbinder waits for a reply that never comes.
`with-logd.sh` now bounds every run with `MOSAIC_TIMEOUT` (120 seconds by
default), and the shim refuses an unparsed stream instead of staying silent.

The probe's own output was also lying at first: it wrote each byte with its own
`write(2)`, so its dumps interleaved with the process's own stderr and the result
looked like a corrupted buffer. It now buffers a line and writes it once. The
bytes below survive that fix, so they are real.

The struct itself reads correctly, which is what makes the framing puzzling:

```
write_size=68  write_consumed=0  write_buffer=0x7f931680ab10
read_size=256  read_consumed=0  read_buffer=0x7f931680bad0
```

`IPCThreadState::writeTransactionData` writes `BC_TRANSACTION` and then the
64-byte `binder_transaction_data`, and `talkWithDriver` sets
`write_buffer = mOut.data()`, so the first four bytes should be `0` and the whole
call should be 68 bytes. The bytes are not that, and the sizes do not decompose
consistently: 68 and 76 differ by 8, and neither `[cmd][tr]` nor
`[prefix][cmd][tr]` accounts for both.

The next attempt should stop reading bytes and check the parcel side instead:
what `Parcel::data()` and `Parcel::ipcData()` return relative to each other, and
whether the outgoing parcel for this call has been through `remove()` or
`setDataSize()` first. Both files are a short fetch away
(`platform/frameworks/native`, `libs/binder/{IPCThreadState,Parcel}.cpp`) and were
read once already without settling it.

## What is left for Phase 3

1. Settle the write-stream framing, then reply to `BINDER_WRITE_READ`: for a
   transaction to handle 0, dispatch it to the service registry and write a
   `BR_REPLY` carrying the resulting handle.
2. Reference counting: `BC_ACQUIRE`/`BC_RELEASE`/`BC_INCREFS`/`BC_DECREFS`, which
   are what keep a remote object alive.
3. Route transactions to handles the registry handed out. For now those services
   do not exist, so the honest answer is an error reply rather than a hang, and
   the framework reports the service as unavailable instead of aborting.
