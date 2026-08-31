#!/usr/bin/env bash
# Fetch the Soroswap contract binaries used by the integration test.
#
#   ./validation/fetch-soroswap.sh [destdir]     # default: validation/vendor
#
# The binaries are downloaded rather than committed: they are third-party
# artifacts, and vendoring someone else's bytecode into this repository would
# be both a supply-chain smell and a redistribution question we do not need to
# answer. Each file is pinned by SHA-256, so a substituted upstream blob fails
# loudly instead of silently changing what the tests exercise.

set -euo pipefail

DEST="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/vendor}"
BASE="https://raw.githubusercontent.com/soroswap/core/main/public/mainnet-deployment-2024-03"

# file                            expected sha256
FILES=(
  "soroswap_router.optimized.wasm  4c3db3ebd2d6a2ab23de1f622eaabb39501539b4611b68622ec4e47f76c4ba07"
  "soroswap_factory.optimized.wasm 5db738b05d9148128a240b0e2c1cb935c2805192bf98a579421aacda364c8dae"
  "soroswap_pair.optimized.wasm    18051456816b66f12e773a56f77c5794fac1b1fb7ab6e22d4fad5a412770f73e"
)

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

mkdir -p "$DEST"
for entry in "${FILES[@]}"; do
  read -r name want <<<"$entry"
  out="$DEST/$name"

  if [ -f "$out" ] && [ "$(sha256 "$out")" = "$want" ]; then
    echo "ok (cached)  $name"
    continue
  fi

  curl -fsSL "$BASE/$name" -o "$out"
  got="$(sha256 "$out")"
  if [ "$got" != "$want" ]; then
    rm -f "$out"
    echo "CHECKSUM MISMATCH  $name" >&2
    echo "  expected $want" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  echo "ok           $name"
done

echo
echo "Soroswap binaries in $DEST"
