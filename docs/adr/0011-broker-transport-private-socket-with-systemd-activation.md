# The broker speaks a private socket protocol, activated by systemd

The broker carries Binder-like traffic: transactions, handles, reference
counts, one-way calls and passed file descriptors. None of that maps cleanly
onto D-Bus, and a bus hop per transaction is the wrong shape for the hottest
path in the system.

Mosaic gives the broker a private Unix domain socket with a framed protocol and
`SCM_RIGHTS` for descriptor passing, and starts it on demand through systemd
socket activation, which also provides the idle-exit behaviour ADR-0005
requires without a bus daemon. D-Bus remains in use only where it is the right
tool, which is polkit authorization for the UID helper in ADR-0008.
