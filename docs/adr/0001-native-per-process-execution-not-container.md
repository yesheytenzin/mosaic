# Native per-process execution instead of a container

Waydroid runs Android apps inside a full LXC-namespaced container booting a
modified Android OS (`/init`, Zygote, System Server). Mosaic instead executes
each Android app as an ordinary host Linux process, translating Android API
calls to host equivalents, with no persistent Android OS instance ever booted.
This matches real desktop-native app behaviour (native Wayland windows, no
separate "Android environment" to boot or manage) rather than sandboxing a whole
guest OS, even though no known FOSS project has done this at this scope.
