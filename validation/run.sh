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

echo "==> installing Trims validation tests"
cp "$HERE"/tests/*.rs test-suites/tests/

echo "==> running"
cargo test -p test-suites --test trims_deleverage_core     -- --nocapture
cargo test -p test-suites --test trims_deleverage_real_amm -- --nocapture

echo
echo "==> both validation tests passed"
