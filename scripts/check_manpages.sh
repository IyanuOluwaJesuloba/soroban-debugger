#!/usr/bin/env bash
# Regenerates man pages into a temp directory and diffs against committed versions.
# Exits non-zero with a clear diff output if drift is detected.
#
# Usage: bash scripts/check_manpages.sh
#   or:  make check-man

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMMITTED_DIR="$REPO_ROOT/man/man1"
TEMP_DIR=$(mktemp -d)

# Always clean up temp dir, even on failure or Ctrl-C
trap 'rm -rf "$TEMP_DIR"' EXIT

TEMP_MAN_DIR="$TEMP_DIR/man/man1"

echo "Generating fresh man pages into $TEMP_DIR..."

# MAN_OUT_DIR is read by build.rs to redirect man page output to a temp directory.
# This avoids touching the committed man/man1/ during the diff check.
MAN_OUT_DIR="$TEMP_MAN_DIR" cargo build --quiet 2>/dev/null

if [ ! -d "$TEMP_MAN_DIR" ]; then
    echo "❌ Man page generation failed: $TEMP_MAN_DIR was not created."
    exit 2
fi

if [ ! -d "$COMMITTED_DIR" ]; then
    echo "❌ Committed man page directory not found: $COMMITTED_DIR"
    echo "   Run 'make regen-man' to generate and commit man pages."
    exit 1
fi

echo "Diffing against committed man pages in $COMMITTED_DIR..."

diff_output=$(diff -r "$COMMITTED_DIR" "$TEMP_MAN_DIR" 2>&1) || diff_status=$?
diff_status=${diff_status:-0}

if [ "$diff_status" -eq 0 ]; then
    echo "✅ Man pages are in sync."
    exit 0
elif [ "$diff_status" -eq 1 ]; then
    echo ""
    echo "❌ Drift detected between committed man pages and current CLI source."
    echo "   Run 'make regen-man' and commit the updated .1 files."
    echo ""
    echo "--- diff output ---"
    echo "$diff_output"
    exit 1
else
    echo "❌ diff exited with error (status $diff_status)."
    echo "$diff_output"
    exit 2
fi
