# A narrow privileged helper allocates per-app UIDs

Real system UIDs require root to create, but the broker is deliberately
unprivileged. Mosaic uses a separate, narrow privileged helper — a
polkit-authorized action scoped to one mode of the `mosaic` binary — whose only
job is allocating a UID in the reserved range, creating the system user and
owning the app's data directory.

The broker owns the allocation decision, because it owns the registry and is
the only writer, but it never performs the privileged step. The install command
does, from the user's session, because a systemd user service has no session for
polkit to prompt on. That makes install a two step transaction: the broker
reserves a UID, the caller creates the user, the broker commits. The
reservation is invisible as an install, so a refused prompt leaves nothing
behind and a retry is the same as a first attempt.

Running the broker as root was rejected: that would make every IPC routing bug
a potential root privilege bug.
