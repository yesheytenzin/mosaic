#!/bin/bash
# B13: a property store that persists.
#
#   tools/verify-persist.sh <runtime-bundle>
#
# The round trip is exercised through the service's own functions against a scratch
# bundle root, so it tests the code the boot runs rather than a re-implementation of
# it: save a `persist.*` value, drop everything in memory, load it back and check it
# is the same. The file's format is ours (`name=value` per line, one per line,
# written to a temporary and renamed), because nothing else reads it.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bundle=${1:?usage: verify-persist.sh <runtime-bundle>}

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/data/property"
mkdir -p "$scratch/properties"

MOSAIC_ANDROID_ROOT="$scratch" MOSAIC_PROPERTY_DIR="$scratch/properties" \
python3 - "$root/tools/bundle/android-property-service.py" <<'PY'
import importlib.util
import os
import sys

path = sys.argv[1]
spec = importlib.util.spec_from_file_location("property_service", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

name = "persist.mosaic.test"
module.persist_save(name, "1")
on_disk = module.persist_path()
assert os.path.exists(on_disk), f"{on_disk} was not written"

loaded = module.persist_load()
assert loaded.get(name) == "1", f"read back {loaded!r}"

# A second save must not lose the first, which is what the rename into place buys.
module.persist_save("persist.mosaic.other", "2")
again = module.persist_load()
assert again.get(name) == "1" and again.get("persist.mosaic.other") == "2", again

# And the file is plain text with one entry per line.
with open(on_disk) as handle:
    body = handle.read()
assert body.count("\n") == 2, body
assert not os.path.exists(on_disk + ".new"), "the temporary was left behind"

print(f"property-service: persist round trip ok ({body.splitlines()[0]!r}, ...)")
PY
