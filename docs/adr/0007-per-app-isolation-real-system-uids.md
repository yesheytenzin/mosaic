# Per-app isolation is committed scope, not deferred

Android's security model gives every app a distinct Linux UID with
kernel-enforced filesystem boundaries. A single shared data prefix, like Wine's
`WINEPREFIX`, would not replicate that. Mosaic pursues per-app isolation
approximating Android's own model rather than deferring the question until the
execution model is proven. The mechanism is real distinct system Linux UIDs,
chosen over unprivileged user namespaces and Landlock despite requiring a
privileged allocation step at install time. Allocation authority belongs to the
broker, obtained as described in ADR-0008.
