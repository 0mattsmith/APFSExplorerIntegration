#!/usr/bin/env bash
# Regenerate the committed APFS test fixture.
# Requires: git, make, gcc, cargo. Linux (mkapfs/apfsck are Linux tools).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "== building apfsprogs (mkapfs + apfsck) =="
git clone --depth 1 https://github.com/linux-apfs/apfsprogs.git "$WORK/apfsprogs"
make -C "$WORK/apfsprogs/mkapfs"  >/dev/null
make -C "$WORK/apfsprogs/apfsck" >/dev/null

echo "== building apfs-fixture =="
cargo build -p apfs-fixture --manifest-path "$ROOT/Cargo.toml"
FIXTURE_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/apfs-fixture"

echo "== formatting + injecting =="
IMG="$WORK/apfs-16m.img"
truncate -s 16M "$IMG"
"$WORK/apfsprogs/mkapfs/mkapfs" -L TestVol "$IMG"
"$FIXTURE_BIN" "$IMG"

echo "== validating with apfsck (independent oracle) =="
"$WORK/apfsprogs/apfsck/apfsck" -c "$IMG"
echo "apfsck: clean"

echo "== committing fixture =="
gzip -9 -c "$IMG" > "$ROOT/crates/apfs-core/tests/fixtures/apfs-16m.img.gz"
ls -la "$ROOT/crates/apfs-core/tests/fixtures/"
echo "done. Run: cargo test -p apfs-core"
