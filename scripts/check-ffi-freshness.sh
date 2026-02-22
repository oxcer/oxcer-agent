#!/usr/bin/env bash
# check-ffi-freshness.sh -- CI/pre-commit freshness guard.
#
# Fails with exit code 1 if the committed oxcer_ffi.swift does not match
# what uniffi-bindgen would generate from the current Rust source.
#
# Mirrors the logic in .github/workflows/ci.yml (uniffi-binding-freshness job)
# so developers can run the same check locally before pushing:
#
#   ./scripts/check-ffi-freshness.sh
#
# It can also be wired as a pre-push git hook:
#   cp scripts/check-ffi-freshness.sh .git/hooks/pre-push
#   chmod +x .git/hooks/pre-push

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMMITTED="$REPO_ROOT/apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift"
DYLIB="$REPO_ROOT/target/release/liboxcer_ffi.dylib"
TMP_DIR="$(mktemp -d)"

cd "$REPO_ROOT"

echo "[step] Building oxcer_ffi (release)..."
cargo build --release -p oxcer_ffi

echo "[step] Generating fresh bindings into $TMP_DIR..."
cargo run --bin uniffi-bindgen -- generate \
    --library "$DYLIB" \
    --language swift \
    --out-dir "$TMP_DIR"

FRESH="$TMP_DIR/oxcer_ffi.swift"

if diff -q "$FRESH" "$COMMITTED" > /dev/null 2>&1; then
    echo "[OK] UniFFI Swift bindings are fresh."
    rm -rf "$TMP_DIR"
    exit 0
fi

echo ""
echo "[ERROR] FFI BINDINGS ARE STALE"
echo "  Committed file : $COMMITTED"
echo "  Fresh output   : $FRESH"
echo ""
echo "  The committed Swift bindings do not match the current Rust source."
echo "  This is the root cause of the 88 GB virtual-memory incident:"
echo "  a stale decoder tries to read a 4-byte i32 as a 24-byte RustBuffer,"
echo "  interprets garbage as an array length, and calls reserveCapacity."
echo ""
echo "  Fix: ./scripts/regen-ffi.sh"
echo ""
echo "--- diff (fresh vs committed) ---"
diff "$FRESH" "$COMMITTED" || true

rm -rf "$TMP_DIR"
exit 1
