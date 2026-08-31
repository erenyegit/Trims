#!/usr/bin/env bash
# Reproduce the Trims validation tests against unmodified Blend V2 contracts.
#
#   ./validation/run.sh [workdir]
#
# Requirements: rust + cargo, the wasm32-unknown-unknown target, git.
#   rustup target add wasm32-unknown-unknown
#
# The stellar CLI is NOT required: Blend's Makefile only uses it to shrink the
# wasm, which contractimport! does not care about, so we copy instead.

set -euo pipefail

WORK="${1:-$(mktemp -d)}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$WORK/blend-contracts-v2"

echo "==> workdir: $WORK"

if [ ! -d "$REPO" ]; then
  git clone --depth 1 https://github.com/blend-capital/blend-contracts-v2.git "$REPO"
fi
cd "$REPO"

echo "==> building contract wasm"
for c in pool-factory backstop pool; do
  cargo rustc --manifest-path="$c/Cargo.toml" --crate-type=cdylib \
              --target=wasm32-unknown-unknown --release
done

mkdir -p target/wasm32-unknown-unknown/optimized
cp target/wasm32-unknown-unknown/release/pool_factory.wasm \
   target/wasm32-unknown-unknown/release/backstop.wasm \
   target/wasm32-unknown-unknown/release/pool.wasm \
   target/wasm32-unknown-unknown/optimized/

echo "==> staging Trims and Soroswap artifacts"
# The integration test runs inside Blend's workspace, so everything it loads has
# to sit at a path `contractimport!` can resolve from the test-suites crate root.
ART="$REPO/trims-artifacts"
mkdir -p "$ART"

if ! command -v stellar >/dev/null; then
  echo "error: the stellar CLI is required." >&2
  echo "  Plain 'cargo build' emits call_indirect immediates as padded LEBs," >&2
  echo "  which the Soroban host rejects on upload. 'stellar contract optimize'" >&2
  echo "  rewrites them. See docs/findings.md, finding 9." >&2
  echo "  Install: brew install stellar-cli" >&2
  exit 1
fi

TRIMS="$HERE/../contracts"
(cd "$TRIMS" && cargo build --release --target wasm32-unknown-unknown)
for c in trims_manager trims_receiver; do
  stellar contract optimize \
    --wasm     "$TRIMS/target/wasm32-unknown-unknown/release/$c.wasm" \
    --wasm-out "$ART/$c.wasm"
done

"$HERE/fetch-soroswap.sh" >/dev/null
cp "$HERE"/vendor/soroswap_*.optimized.wasm "$ART/"

echo "==> installing Trims validation tests"
cp "$HERE"/tests/*.rs test-suites/tests/

echo "==> running"
cargo test -p test-suites --test trims_deleverage_core        -- --nocapture
cargo test -p test-suites --test trims_deleverage_real_amm    -- --nocapture
cargo test -p test-suites --test trims_integration_soroswap   -- --nocapture

echo
echo "==> all validation tests passed"
