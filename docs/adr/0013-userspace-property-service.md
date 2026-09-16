# A userspace property service, and the property area

ART and `app_process` read Android system properties throughout startup:
`app_process` aborts without `ro.product.cpu.abilist64`, ART wants
`dalvik.vm.*` and the SDK level for almost every decision it makes. Bionic does
not ask a daemon for these values. It maps a shared memory area — on Android,
the files under `/dev/__properties__/` that `init` creates — and reads them
directly. A host process without that area gets an empty value for every
property, and `app_process` stops there:

```
app_process: Unable to determine ABI list from property ro.product.cpu.abilist64.
```

So Mosaic has to provide both halves: somewhere to store property values, and
the area Bionic will look in.

Mosaic implements the property service itself, in the broker. Properties are a
framework service and the broker already owns the framework services (ADR-0005);
on a device they are `init`'s, and nothing about them needs a kernel facility.
The bundle's `ro.*` values come from the image's `build.prop`, and `dalvik.vm.*`
values are Mosaic's to choose, because Mosaic decides how ART runs.

The area is provisioned once, by the narrow privileged helper (ADR-0008), which
is the same shape as UID allocation: one root step, at a known time, doing one
thing. The broker then writes values through a socket the helper's step created
writable, so no further privilege is needed.

The alternative — build Bionic with the property area path configurable, since
ADR-0002 has Mosaic building Bionic anyway — is strictly better in the long run
and strictly more work now, because building Bionic needs an AOSP tree this
project does not have yet. It stays available: nothing here depends on the area
being at `/dev/__properties__` except the provisioning step, and swapping it
later touches one function.
