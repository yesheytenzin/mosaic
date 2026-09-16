# Mosaic

Runs Android apps as native Linux processes rather than inside a container. The
Android API surface, framework services, Binder and the hardware abstraction
layer are translated to host equivalents, and no Android OS ever boots.

The project grew out of a Rust port of Waydroid's container model. That port is
being retired: containers boot a whole Android system to run one app, need a
kernel binder driver many hosts do not have, and put the app in a separate
"Android environment" instead of on the desktop. See `docs/adr/` for the
decisions and their reasoning.

## Language

**Native execution**:
An Android app runs as an ordinary host process. Its DEX bytecode executes on a
host-native ART runtime and its native code links against a host-native Bionic,
with the framework, Binder and HAL layer reimplemented in userspace.
_Avoid_: container execution, Android environment

**Container execution**:
The model this replaces: a full modified Android system boots inside an LXC
namespace and the app runs inside it, reached from the host over kernel binder.
This is what Waydroid does and what the retired port did.
_Avoid_: native execution

**Runtime bundle**:
The pinned, hash-verified archive of the host-native ART and Bionic build. The
app process is built from it. It is downloaded and cached the same way the old
container images were, but it contains no Android OS.
_Avoid_: system image, vendor image

**Broker**:
The single, lazily started, idle-exiting unprivileged process that owns the
package registry, on-demand app launching, userspace Binder routing, and the
Android-specific window policy that has no Wayland equivalent. Analogous to
Wine's `wineserver`. It is not a System Server: there is no boot sequence and no
persistent Android OS.
_Avoid_: System Server, session manager

**Handle**:
A process-local reference to a userspace Binder object, carrying an owner and a
node identity. Handles are what the userspace Binder library passes between
processes, in place of the kernel driver's node and reference tables.
_Avoid_: file descriptor, fd (unless a real host fd is meant)

**App state machine**:
`Installed -> Starting -> Running -> Exited`, owned by the broker. This replaces
reading a container's status from an out-of-band string, which was the source of
a real misdiagnosis in the retired port.
_Avoid_: container status
