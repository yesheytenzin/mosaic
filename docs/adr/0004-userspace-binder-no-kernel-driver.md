# Binder reimplemented in userspace, no kernel driver dependency

Waydroid depends on a kernel Binder driver (`binder_linux`, binderfs and the
hwbinder and vndbinder nodes). That requirement is also what made the retired
container port unrunnable on hosts whose kernel does not ship the driver, and
what left guest processes in uninterruptible sleep when an out-of-tree driver
misbehaved. Mosaic reimplements Binder's IPC semantics as a userspace library
instead, dropping the kernel module dependency entirely. This means re-solving
reference counting, death notification and transaction semantics that the kernel
driver provided, which is accepted as the cost of removing a kernel-side
dependency many target hosts will not have.
