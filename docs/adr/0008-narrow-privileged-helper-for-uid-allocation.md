# A narrow privileged helper allocates per-app UIDs

Real system UIDs require root to create, but the broker is deliberately
unprivileged. Mosaic uses a separate, narrow privileged helper, a
polkit-authorized action or a minimal setuid binary, whose only job is
allocating the next UID in a reserved range, creating the system user and
chowning the app's data directory. The broker invokes it at install time,
authorized through the desktop's polkit agent. Running the broker as root was
rejected: that would make every IPC routing bug a potential root privilege bug.
