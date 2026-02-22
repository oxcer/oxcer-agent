#!/usr/bin/env bash
# regen-ffi.sh -- Regenerate Swift UniFFI bindings and copy into the Xcode project.
#
# Always uses a release build so the dylib matches what Xcode links at runtime.
# (Using a debug dylib for bindgen while Xcode links release is the primary
# cause of apiChecksumMismatch at launch — see docs/ffi-migration.md.)
#
# Run this every time you change a Rust function signature, add/remove an
# #[uniffi::export] item, or modify any #[uniffi::Record] / #[uniffi::Error].
#
# After running:
#   1. Build the Xcode target (Cmd+B) to confirm the app compiles.
#   2. Run OxcerFFITests (Cmd+U) -- especially the memory sentinel.
#   3. Commit both the Rust change and the regenerated oxcer_ffi.swift together.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_SWIFT_DIR="$REPO_ROOT/apps/OxcerLauncher/OxcerLauncher"
DEST_FILE="$APP_SWIFT_DIR/oxcer_ffi.swift"
STALE_GENERATED_DIR="$REPO_ROOT/generated_swift"
DYLIB_PATH="$REPO_ROOT/target/release/liboxcer_ffi.dylib"
TMP_DIR="$(mktemp -d)"

echo "---"
echo "oxcer FFI binding regeneration (release)"
echo "Dylib : $DYLIB_PATH"
echo "Dest  : $DEST_FILE"
echo "---"

# -- 1. Build the release dylib -----------------------------------------------
echo ""
echo "[step] cargo build --release -p oxcer_ffi"
cd "$REPO_ROOT"
cargo build --release -p oxcer_ffi

if [[ ! -f "$DYLIB_PATH" ]]; then
    echo "[ERROR] Dylib not found at $DYLIB_PATH -- build may have failed."
    exit 1
fi

# -- 2. Run uniffi-bindgen against the release dylib --------------------------
echo ""
echo "[step] uniffi-bindgen generate -> $TMP_DIR"
cargo run --bin uniffi-bindgen -- generate \
    --library "$DYLIB_PATH" \
    --language swift \
    --out-dir "$TMP_DIR"

FRESH="$TMP_DIR/oxcer_ffi.swift"
if [[ ! -f "$FRESH" ]]; then
    echo "[ERROR] uniffi-bindgen did not produce oxcer_ffi.swift in $TMP_DIR"
    ls "$TMP_DIR" || true
    exit 1
fi

# -- 3. Diff (informational) --------------------------------------------------
echo ""
if diff -q "$FRESH" "$DEST_FILE" > /dev/null 2>&1; then
    echo "[OK] Bindings are already up-to-date."
    rm -rf "$TMP_DIR"
    # Still run the checksum gate to catch a stale release dylib.
    _VERIFY_ONLY=1
else
    echo "Changes in generated bindings:"
    diff "$FRESH" "$DEST_FILE" || true

    # -- 4. Copy into Xcode project -------------------------------------------
    echo ""
    echo "[step] Copying fresh bindings -> $DEST_FILE"
    cp "$FRESH" "$DEST_FILE"
    _VERIFY_ONLY=0
fi

# -- 5. Verify: runtime checksums in the release dylib must match the binding -
#
# This catches the exact failure mode that caused the original apiChecksumMismatch:
# bindgen run against debug dylib while Xcode links the stale release dylib.
#
# We extract the expected checksums from the freshly installed binding, then
# call the corresponding C symbols in the release dylib and compare.
echo ""
echo "[step] Verifying release dylib checksums match installed binding..."

if command -v python3 > /dev/null 2>&1; then
    python3 - "$DYLIB_PATH" "$DEST_FILE" <<'PYEOF'
import sys, ctypes, re

dylib_path, swift_path = sys.argv[1], sys.argv[2]

# Parse expected checksums from the Swift binding
expected = {}
with open(swift_path) as f:
    for line in f:
        m = re.search(r'uniffi_oxcer_ffi_checksum_(\w+)\(\) != (\d+)', line)
        if m:
            expected[f'uniffi_oxcer_ffi_checksum_{m.group(1)}'] = int(m.group(2))
        m2 = re.search(r'let bindings_contract_version = (\d+)', line)
        if m2:
            expected['ffi_oxcer_ffi_uniffi_contract_version'] = int(m2.group(1))

lib = ctypes.CDLL(dylib_path)
failures = []
for sym, exp in sorted(expected.items()):
    fn = getattr(lib, sym)
    fn.restype = ctypes.c_uint16
    got = fn()
    label = sym.replace('uniffi_oxcer_ffi_checksum_', '').replace('ffi_oxcer_ffi_', '')
    if got == exp:
        print(f'  [OK]      {label}: {got}')
    else:
        print(f'  [MISMATCH] {label}: dylib={got}  swift_expects={exp}')
        failures.append((label, got, exp))

print()
if failures:
    print('[ERROR] Checksum mismatch detected between release dylib and Swift binding.')
    print('        The release dylib is out of date with the current Rust source.')
    print('        This script should have rebuilt it -- check cargo output above.')
    sys.exit(1)
else:
    print('[OK] All checksums match. uniffiEnsureInitialized() will return .ok')
PYEOF
else
    echo "WARNING: python3 not found -- skipping checksum verification."
    echo "         Run the following manually to confirm no mismatch:"
    echo "           python3 scripts/check-ffi-freshness.sh"
fi

# -- 6. Warn about stale generated_swift/ directory ---------------------------
if [[ -d "$STALE_GENERATED_DIR" ]]; then
    echo ""
    echo "WARNING: $STALE_GENERATED_DIR still exists."
    echo "  This directory is stale and was the source of the 88 GB VM bug."
    echo "  Delete it:"
    echo "    git rm -r $STALE_GENERATED_DIR"
fi

# -- 7. Remind about the full workflow ----------------------------------------
echo ""
echo "[OK] regen-ffi complete."
echo ""
echo "Next steps:"
echo "  1. In Xcode: Product > Clean Build Folder (Shift+Cmd+K), then Build (Cmd+B)."
echo "  2. Run OxcerFFITests (Cmd+U) -- check the VM sentinel passes."
echo "  3. Commit the Rust change and the binding together:"
echo "       git add oxcer_ffi/src/lib.rs $DEST_FILE"
echo "       git commit -m 'ffi: <describe the contract change>'"

rm -rf "$TMP_DIR"
