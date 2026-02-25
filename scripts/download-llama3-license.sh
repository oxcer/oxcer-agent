#!/usr/bin/env bash
# scripts/download-llama3-license.sh
#
# Downloads the verbatim Meta Llama 3 Community License Agreement from Meta's
# official GitHub repository and writes it to the two required locations:
#
#   1. LLAMA3_LICENSE.txt          (repo root — committed to git)
#   2. apps/OxcerLauncher/OxcerLauncher/LLAMA3_LICENSE.txt
#                                  (bundled into the app via Xcode Resources)
#
# Run this once before your first release build and commit both files.
# Re-run if Meta ever updates the license text.
#
# Usage:
#   ./scripts/download-llama3-license.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LICENSE_URL="https://raw.githubusercontent.com/meta-llama/llama3/main/LICENSE"
DEST_ROOT="${REPO_ROOT}/LLAMA3_LICENSE.txt"
DEST_BUNDLE="${REPO_ROOT}/apps/OxcerLauncher/OxcerLauncher/LLAMA3_LICENSE.txt"

echo "Downloading Meta Llama 3 Community License from:"
echo "  ${LICENSE_URL}"
echo ""

curl --fail --silent --show-error --location \
    "${LICENSE_URL}" \
    --output "${DEST_ROOT}"

# Verify the file is non-empty and looks like the real license
if ! grep -q "LLAMA 3 COMMUNITY LICENSE AGREEMENT" "${DEST_ROOT}" 2>/dev/null; then
    echo "ERROR: Downloaded file does not appear to contain the Llama 3 license text."
    echo "       Check the URL or download manually from https://llama.meta.com/llama3/license/"
    exit 1
fi

cp "${DEST_ROOT}" "${DEST_BUNDLE}"

echo "License text written to:"
echo "  ${DEST_ROOT}"
echo "  ${DEST_BUNDLE}"
echo ""
echo "Next steps:"
echo "  1. In Xcode: right-click OxcerLauncher group → Add Files → select"
echo "     apps/OxcerLauncher/OxcerLauncher/LLAMA3_LICENSE.txt"
echo "     Ensure 'Add to targets: OxcerLauncher' is checked."
echo "     (Skip if it is already in Build Phases → Copy Bundle Resources.)"
echo "  2. git add LLAMA3_LICENSE.txt apps/OxcerLauncher/OxcerLauncher/LLAMA3_LICENSE.txt"
echo "  3. git commit -m 'legal: add Meta Llama 3 Community License text'"
