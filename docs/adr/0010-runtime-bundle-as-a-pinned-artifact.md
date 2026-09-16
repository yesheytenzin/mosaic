# The ART and Bionic build ships as a pinned, hash-verified bundle

Building AOSP inside a package build takes hours and needs a full source
checkout, which no distribution would accept. Mosaic instead publishes the
host-native ART and Bionic build as a versioned archive, the runtime bundle, and
downloads it at first use with a pinned hash.

This reuses machinery the retired container port already had: an HTTP client
with a persistent cache and a sha256 check, which used to fetch a system image
and now fetches the runtime bundle. Building it is a separate pipeline from the
package build, and the package depends only on the pinned artifact.
