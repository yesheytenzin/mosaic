# A single lazily started, idle-exiting broker resolves cross-app rendezvous

Binder's design assumes a shared service registry, which is structurally
incompatible with "no persistent process". Cross-app intent resolution and
on-demand app launching need a rendezvous point. Mosaic introduces one broker
process, socket-activated on the first cross-app call and exiting once no apps
are running, which owns IPC routing, the package and service registry,
launch-on-demand, and the Android-specific window policy that has no Wayland
equivalent. It is analogous to Wine's `wineserver`, and it resolves the
rendezvous requirement without reintroducing a persistent Android OS.
