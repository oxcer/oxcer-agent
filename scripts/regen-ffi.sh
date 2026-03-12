#!/usr/bin/env bash
# regen-ffi.sh -- Regenerate Swift UniFFI bindings and copy into the Xcode project.
#
# Always uses a release build so the dylib matches what Xcode links at runtime.
# (Using a debug dylib for bindgen while Xcode links release is the primary
# cause of apiChecksumMismatch at launch — see docs/ffi-migration.md.)
#
# uniffi-bindgen produces THREE files for Swift:
#   oxcer_ffi.swift     -- Swift wrappers (call-site API)
#   oxcer_ffiFFI.h      -- C declarations imported by OxcerLauncher-Bridging-Header.h
#   oxcer_ffiFFI.modulemap -- module map (not imported by Xcode; not copied)
#
# ALL THREE must stay in sync with the release dylib.  Previously only oxcer_ffi.swift
# was copied; the missing header copy caused "Cannot find uniffi_oxcer_ffi_fn_func_*
# in scope" build errors whenever new #[uniffi::export] items were added.
#
# Run this every time you change a Rust function signature, add/remove an
# #[uniffi::export] item, or modify any #[uniffi::Record] / #[uniffi::Error].
#
# After running:
#   1. Build the Xcode target (Cmd+B) to confirm the app compiles.
#   2. Run OxcerFFITests (Cmd+U) -- especially the memory sentinel.
#   3. Commit the Rust change and the two generated files together:
#        git add oxcer_ffi/src/lib.rs \
#                apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift \
#                apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h
#        git commit -m 'ffi: <describe the contract change>'

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_SWIFT_DIR="$REPO_ROOT/apps/OxcerLauncher/OxcerLauncher"
DEST_SWIFT="$APP_SWIFT_DIR/oxcer_ffi.swift"
DEST_HEADER="$APP_SWIFT_DIR/oxcer_ffiFFI.h"
STALE_GENERATED_DIR="$REPO_ROOT/generated_swift"
DYLIB_PATH="$REPO_ROOT/target/release/liboxcer_ffi.dylib"
TMP_DIR="$(mktemp -d)"

echo "---"
echo "oxcer FFI binding regeneration (release)"
echo "Dylib  : $DYLIB_PATH"
echo "Swift  : $DEST_SWIFT"
echo "Header : $DEST_HEADER"
echo "---"

# -- 1. Build the release dylib -----------------------------------------------
echo ""
echo "[step] cargo build --locked --release -p oxcer_ffi"
cd "$REPO_ROOT"
cargo build --locked --release -p oxcer_ffi

if [[ ! -f "$DYLIB_PATH" ]]; then
    echo "[ERROR] Dylib not found at $DYLIB_PATH -- build may have failed."
    exit 1
fi

# -- 2. Run uniffi-bindgen against the release dylib --------------------------
echo ""
echo "[step] uniffi-bindgen generate -> $TMP_DIR"
cargo run --locked --bin uniffi-bindgen -- generate \
    --library "$DYLIB_PATH" \
    --language swift \
    --out-dir "$TMP_DIR"

FRESH_SWIFT="$TMP_DIR/oxcer_ffi.swift"
FRESH_HEADER="$TMP_DIR/oxcer_ffiFFI.h"

if [[ ! -f "$FRESH_SWIFT" ]]; then
    echo "[ERROR] uniffi-bindgen did not produce oxcer_ffi.swift in $TMP_DIR"
    ls "$TMP_DIR" || true
    exit 1
fi
if [[ ! -f "$FRESH_HEADER" ]]; then
    echo "[ERROR] uniffi-bindgen did not produce oxcer_ffiFFI.h in $TMP_DIR"
    ls "$TMP_DIR" || true
    exit 1
fi

# -- 3. Diff both generated files (informational) -----------------------------
echo ""
SWIFT_CHANGED=0
HEADER_CHANGED=0

if diff -q "$FRESH_SWIFT" "$DEST_SWIFT" > /dev/null 2>&1; then
    echo "[OK] oxcer_ffi.swift is already up-to-date."
else
    echo "Changes in oxcer_ffi.swift:"
    diff "$FRESH_SWIFT" "$DEST_SWIFT" || true
    SWIFT_CHANGED=1
fi

echo ""
if diff -q "$FRESH_HEADER" "$DEST_HEADER" > /dev/null 2>&1; then
    echo "[OK] oxcer_ffiFFI.h is already up-to-date."
else
    echo "Changes in oxcer_ffiFFI.h:"
    diff "$FRESH_HEADER" "$DEST_HEADER" || true
    HEADER_CHANGED=1
fi

# -- 4. Copy changed files into Xcode project ---------------------------------
echo ""
if [[ $SWIFT_CHANGED -eq 0 && $HEADER_CHANGED -eq 0 ]]; then
    echo "[OK] All bindings are already up-to-date. No files copied."
else
    echo "[step] Copying updated bindings..."
    if [[ $SWIFT_CHANGED -eq 1 ]]; then
        cp "$FRESH_SWIFT" "$DEST_SWIFT"
        echo "  copied -> $DEST_SWIFT"
    fi
    if [[ $HEADER_CHANGED -eq 1 ]]; then
        cp "$FRESH_HEADER" "$DEST_HEADER"
        echo "  copied -> $DEST_HEADER"
    fi
fi

rm -rf "$TMP_DIR"

# -- 5. Sanity-check: installed header must contain ffi_agent_step symbols ----
#
# Grep the installed header for the two C declarations that Swift's compiler
# needs to resolve ffi_agent_step calls.  This is warn-only — the symbols may
# legitimately be absent in a future refactor, but a missing symbol is almost
# always an accidental header regression.
echo ""
echo "[step] Sanity-checking installed header for ffi_agent_step symbols..."
WARN_HEADER=0

if ! grep -q "uniffi_oxcer_ffi_fn_func_ffi_agent_step" "$DEST_HEADER" 2>/dev/null; then
    echo "  WARN: 'uniffi_oxcer_ffi_fn_func_ffi_agent_step' not found in $DEST_HEADER"
    WARN_HEADER=1
fi
if ! grep -q "uniffi_oxcer_ffi_checksum_func_ffi_agent_step" "$DEST_HEADER" 2>/dev/null; then
    echo "  WARN: 'uniffi_oxcer_ffi_checksum_func_ffi_agent_step' not found in $DEST_HEADER"
    WARN_HEADER=1
fi

if [[ $WARN_HEADER -eq 0 ]]; then
    echo "  [OK] ffi_agent_step symbols present in installed header."
fi

# -- 6. Verify: runtime checksums in the release dylib must match the binding -
#
# Parses expected checksums from oxcer_ffi.swift, calls the C symbols in the
# release dylib via ctypes, and fails loudly on any mismatch.
#
# Catches: stale release dylib, or bindgen run against the wrong dylib.
echo ""
echo "[step] Verifying release dylib checksums match installed binding..."

if command -v python3 > /dev/null 2>&1; then
    python3 - "$DYLIB_PATH" "$DEST_SWIFT" <<'PYEOF'
import sys, ctypes, re

dylib_path, swift_path = sys.argv[1], sys.argv[2]

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
fi

# -- 7. Warn about stale generated_swift/ directory ---------------------------
if [[ -d "$STALE_GENERATED_DIR" ]]; then
    echo ""
    echo "WARNING: $STALE_GENERATED_DIR still exists."
    echo "  This directory is stale and was the source of the 88 GB VM bug."
    echo "  Delete it: git rm -r $STALE_GENERATED_DIR"
fi

# -- 8. Remind about the full workflow ----------------------------------------
echo ""
echo "[OK] regen-ffi complete."
echo ""
echo "Next steps:"
echo "  1. In Xcode: Product > Clean Build Folder (Shift+Cmd+K), then Build (Cmd+B)."
echo "  2. Run OxcerFFITests (Cmd+U) -- check the VM sentinel passes."
echo "  3. Commit the Rust change and BOTH generated files together:"
echo "       git add oxcer_ffi/src/lib.rs \\"
echo "               apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift \\"
echo "               apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h"
echo "       git commit -m 'ffi: <describe the contract change>'"
