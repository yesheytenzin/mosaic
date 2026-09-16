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

A fourth attempt dumped a window *around* `write_buffer` rather than reading from
it, on the theory that the stream might not begin at the pointer given. It does
not, and the buffer is not a command stream at all:

```
 -16: 0b 01 10 00 00 00 ca 88 00 00 00 00 00 00 00 00
   0: 00 63 40 40 00 00 00 00 00 00 00 00 00 00 00 00
  16: 00 00 00 00 47 4e 50 5f 10 00 00 00 00 00 00 00
  32: 00 00 00 00 ... (all zeros to +160)
```

`write_size` is 68, which is exactly one command plus one `binder_transaction_data`,
and the transaction for `checkService` must carry non-zero data pointers. This
buffer has none: it is zeros with two small islands, the first word is
`0x000040406300`, and that word is **not a readable address** either. So it is
neither a command stream nor a pointer. Every reading of the receiving side has
now been ruled out with evidence, which is the strongest argument for the next
attempt being on the sender: log `IPCThreadState::mOut`'s size and data pointer
from inside `talkWithDriver`, or interpose the Parcel writes, and compare with
what arrives here. The window dump is worth recreating (non-destructively parsing
`/proc/self/maps` once a query, since writing a NUL over each line's dash made
every query after the first report mapped memory as unmapped).

A third attempt tried the reply itself, guessing that the stream begins four bytes
in (the word there parses as a transaction to handle 0 with a plausible code). It
does not: after skipping four bytes only 64 remain, and a transaction needs 68, so
the loop broke before finding one, no reply was written, and libbinder retried
until the run was killed -- having written a 5.5 GB log first. Two things follow.
The offset theory is dead: a 68-byte stream cannot both start at 4 and contain a
command plus a transaction. And the harness now caps output as well as time
(`MOSAIC_MAX_OUTPUT`), because that was the second five-gigabyte log this project
has produced and the first fix only bounded the clock.

## The way around it

The driver protocol does not have to be reproduced. `libbinder` exports the level
above it, with declared arguments instead of an opaque buffer:

```
android::IPCThreadState::transact(int, unsigned int, android::Parcel const&,
                                  android::Parcel*, unsigned int)@@LIBBINDER
android::BpBinder::transact(unsigned int, android::Parcel const&,
                            android::Parcel*, unsigned int)@@LIBBINDER
```

A shim can interpose `IPCThreadState::transact` and implement the *semantics* --
handle 0 is the service manager, a code is a call, the replies are Parcels -- using
`Parcel`'s own exported methods. That is a far smaller surface than the driver
protocol, and it is exactly what a binder shim in a container-style project does.
The kernel driver is only a transport between two processes; replacing it at the
API level skips the part that has resisted four attempts at decoding.

The trade is that the shim then lives at `libbinder`'s version of the ABI rather
than at the driver's, which is stable in practice but is a foreign C++ ABI: the
calls have to be made through `dlsym`ed symbols with hand-written signatures, as
the launcher already does for `JNI_CreateJavaVM`.

## Binder at the API level works

`tools/binder-shim/android-binder.c` interposes
`BpBinder::transact` -- defined under its mangled name, so a preload wins for
callers outside libbinder, which is how the framework reaches binder -- and
answers the service manager in userspace using `Parcel`'s exported writers. It
takes the first three IServiceManager calls:

- `checkService`/`getService`: a zero exception code and a null binder, which is a
  valid reply for an absent service. The framework moves past
  `DisplayManagerGlobal`, where it had been failing.
- `addService`: success. The framework then **starts registering its own
  services**, which is `startBootstrapServices` running.

```
android-binder: transact handle=12 code=2 flags=2
android-binder: answered "no such service"
android-binder: transact handle=12 code=3 flags=2
android-binder: accepted a service registration
```

Two things learned from the kernel and from running it. A reply must carry the
answered transaction's `code` and `flags` and address the read buffer with its
data pointers (`drivers/android/binder.c`, `binder_thread_read`); and `BpBinder`'s
handle is not at a fixed offset because `IBinder` derives virtually from
`RefBase` -- it does not matter yet, since with no services the only reachable
target is handle 0, but it will when a second binder exists.

The next step is the registry this stops short of: store the binder that
`addService` carries, hand one back on `getService`, and route a transaction to a
handle by calling the stored `BBinder::transact`, which is exported too. That is a
complete minimal userspace binder, and it is what makes the framework's own
services reachable.

## The framing, settled

Every `BC_*` and `BR_*` constant in `binder.h` is an `_IO`-style encoding, not a
small integer:

```c
#define BC_TRANSACTION  _IOW('c', 0, struct binder_transaction_data)
#define BC_DECREFS      _IOW('c', 5, __u32)
#define BR_REPLY        _IOR('c', 1, struct binder_transaction_data)
```

So the command word carries its own argument length in bits 16..29, and the
stream is self-describing: read a word, take that many bytes, repeat. Earlier
attempts read the word as a command *number*, which is why the bytes "looked like
they were not a command word" -- they were, and the two are decodable exactly:

```
write_size=68  [00 63 40 40 ...]  == 0x40406300 == _IOW('c', 0, 64) == BC_TRANSACTION
                                  68 == 4 + 64, the size field
write_size=8   [05 63 04 40 00 00 00 00] == 0x40046305 == _IOW('c', 5, 4) == BC_DECREFS
                                  8 == 4 + 4
```

The trailing `47 4e 50 5f` that looked like text is the last four bytes of the
64-byte `binder_transaction_data`, which is the size the constant itself declares.
The two calls differ because they *are* different commands; that was never a
puzzle.

What that leaves is ordinary work rather than a mystery: parse the stream, dispatch
`BC_TRANSACTION` for handle 0 through the service registry that
`tools/binder-shim/android-binder.c` already implements at the parcel level, and
answer with `BR_REPLY` (0x80406301) plus a `binder_transaction_data` whose
`data.ptr.buffer` addresses a reply parcel this side allocates. The framework
returns that buffer with `BC_FREE_BUFFER`, which is where it gets freed, and
`read_consumed` has to be set to what was written.

That is the only way the *Java* path can work. Its calls are
`BinderProxy.transact` -> `IBinder::transact` -> `BpBinder::transact` ->
`IPCThreadState::transact`, all inside libbinder, so every one of them is a local
bind under `-fno-semantic-interposition` and no preload can see them; the API-level
interposition that made the AIDL path work cannot reach it. What the Java path does
reach is `ioctl`, and that is why the framework reports every service as missing:

```
SystemServiceRegistry: No service published for: appops
java.os.ServiceManager$ServiceNotFoundException
	at com.android.server.power.PowerManagerService$Injector.createAppOpsManager
	at com.android.server.power.PowerManagerService.<init>
System: ************ Failure starting system services
```

`AppOpsService published` appears earlier in the same log, so the registration was
made and then lost: the publication is a Java call, so it went to the driver, where
the shim answered a read with an empty parcel, which libbinder reads as a null
binder. The registry was never involved.

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

## The Java path, now answered

The wall above was real but the conclusion drawn from it was wrong. The constant
values were read out of libbinder's own machine code rather than recalled:

```
BC_TRANSACTION  _IOW('c', 0, 64)  = 0x40406300   the observed 68-byte stream
BC_FREE_BUFFER  _IOW('c', 3,  8)  = 0x40086303
BR_REPLY        _IOR('r', 3, 64)  = 0x80407203
BR_TRANSACTION_COMPLETE _IOR('r',4,4) = 0x80047204
BR_NOOP         _IO('r', 12)      = 0x0000720c
```

`IPCThreadState::writeTransactionData` was disassembled too, which settles
`binder_transaction_data` at 64 bytes with the code at 16, the sizes at 32 and 40,
and the two pointers at 48 and 56 -- and the command word's own size field agrees,
which is what makes the stream self-describing.

Two more things the running framework taught, both now implemented:

- **The interface token is not at the start of the request.** This build prefixes
  it with twelve bytes and puts an int32 between it and the service name, so
  skipping one string16 from the front reads the prefix, and taking the next one
  reads an empty string. Searching for the descriptor and then for the first
  printable string16 after it is what finds the name -- and checking the
  descriptor is also what stops one interface's transaction code 2 from being
  answered as `checkService`.
- **A waiting thread must not be given nothing.** Returning with an empty read
  buffer makes libbinder read command 0 out of it and log `*** BAD COMMAND 0
  received from Binder driver`. `BR_NOOP` is the answer: its command loop treats
  it as "nothing happened, ask again".

With that, the framework's Java calls reach the registry: names are read
correctly, registrations are remembered, and lookups find them --

```
android-binder: AIDL register memtrack.proxy
android-binder: checkService memtrack.proxy found
```

-- which is the first time any service lookup in this project has returned
anything. `BpBinder::transact` *is* reachable for these calls after all, so the
driver door turns out to be needed for the HIDL half (`/dev/hwbinder`, where
`defaultServiceManager() is null`) rather than for the Java service manager.

## What is left for Phase 3

The shim answers the service manager and hands back local objects, so a caller in
the same process needs no further routing. Two things follow from that:

1. A transaction for a handle -- an object in *another* process -- is the
   broker's to carry. `src/binder/transport.rs` does that between two
   connections; what is missing is the shim as a client of it, and a service that
   exists to be reached.
2. `/dev/hwbinder` needs its own service manager, the way `/dev/binder` has one,
   for the HIDL calls that today find `defaultServiceManager() is null`.
