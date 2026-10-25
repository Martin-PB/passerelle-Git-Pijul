#!/usr/bin/env bash
# scripts/setup-lean.sh - install elan + the pinned Lean toolchain and build Bridge.
#
# Ubuntu (and other Linux): installs elan (the Lean toolchain manager, same
# role as rustup for Rust), lets elan download the exact toolchain pinned in
# lean-toolchain, then runs `lake build Bridge`.
#
# Usage (from the repository root):
#     bash scripts/setup-lean.sh              # install elan if needed, then build
#     bash scripts/setup-lean.sh --no-install # assume elan is present, just build
#
# Exit codes (same convention as scripts/verify-local.sh):
#     0 = all steps passed
#     1 = a step failed
#     2 = a prerequisite is missing (nothing was installed)
#
# The script never runs sudo. If a system package is missing it prints the
# exact apt command for you to run, then exits.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ELAN_BIN="$HOME/.elan/bin"
PASS=0
FAIL=0
SKIP=0
NO_INSTALL=0

step() { printf '\n== %s ==\n' "$1"; }
ok()   { printf 'PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
ko()   { printf 'FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip() { printf 'SKIP: %s\n' "$1"; SKIP=$((SKIP + 1)); }

if [ "${1:-}" = "--no-install" ]; then
    NO_INSTALL=1
elif [ $# -gt 0 ]; then
    echo "Unknown option: $1 (only --no-install is supported)"
    exit 2
fi

# --- Preflight -------------------------------------------------------------

step "Preflight: system prerequisites"

MISSING_PKGS=""
have() { command -v "$1" >/dev/null 2>&1; }

if have curl; then ok "curl present"; else MISSING_PKGS="$MISSING_PKGS curl"; fi
if have git; then ok "git present"; else MISSING_PKGS="$MISSING_PKGS git"; fi
if have cc || have gcc || have clang; then
    ok "C compiler present (required to build Lean)"
else
    MISSING_PKGS="$MISSING_PKGS build-essential"
fi

if [ -n "$MISSING_PKGS" ]; then
    echo "Missing system packages:$MISSING_PKGS"
    echo "Run this yourself (needs sudo), then re-run this script:"
    echo "    sudo apt-get update && sudo apt-get install -y$MISSING_PKGS"
    exit 2
fi

if [ ! -f "$ROOT/lean-toolchain" ]; then
    echo "lean-toolchain file missing at repository root - cannot pin the toolchain."
    exit 1
fi
PINNED="$(cat "$ROOT/lean-toolchain")"
ok "pinned toolchain: $PINNED"

if [ ! -f "$ROOT/lakefile.lean" ]; then
    echo "lakefile.lean missing at repository root - nothing to build."
    exit 1
fi

# --- elan --------------------------------------------------------------------

step "elan (Lean toolchain manager)"

if [ "$NO_INSTALL" -eq 1 ]; then
    if [ -x "$ELAN_BIN/elan" ]; then
        ok "elan assumed present (--no-install): $("$ELAN_BIN/elan" --version)"
    else
        ko "elan not found at $ELAN_BIN/elan and --no-install given"
        exit 1
    fi
elif [ -x "$ELAN_BIN/elan" ]; then
    ok "elan already installed: $("$ELAN_BIN/elan" --version)"
else
    echo "Installing elan (no sudo needed, installs under ~/.elan)..."
    if curl -sSfL https://elan.lean-lang.org/elan-init.sh | sh -s -- -y --default-toolchain none; then
        ok "elan installed"
    else
        ko "elan installation failed"
        exit 1
    fi
fi

export PATH="$ELAN_BIN:$PATH"

if ! have elan; then
    ko "elan not on PATH after install - add $ELAN_BIN to PATH manually and re-run"
    exit 1
fi

if [ -n "$MISSING_RC_NOTE" ]; then :; fi
if ! grep -q '.elan/bin' "$HOME/.profile" 2>/dev/null && ! grep -q '.elan/bin' "$HOME/.bashrc" 2>/dev/null; then
    printf 'NOTE: add elan to your PATH permanently, e.g. in ~/.bashrc:\n    export PATH="%s:$PATH"\n' "$ELAN_BIN"
fi

# --- Toolchain --------------------------------------------------------------

step "Lean toolchain (downloaded by elan on first use, needs network)"

if lean --version >/dev/null 2>&1; then
    ok "lean $(lean --version)"
else
    ko "lean not usable - check the elan install and the lean-toolchain file"
    exit 1
fi
if lake --version >/dev/null 2>&1; then
    ok "lake $(lake --version)"
else
    ko "lake not usable - check the elan install"
    exit 1
fi

ACTUAL="$(elan which lean >/dev/null 2>&1 && echo ok || echo unknown)"
echo "info: elan resolves the pinned toolchain automatically from lean-toolchain"

# --- Build -------------------------------------------------------------------

step "lake build Bridge"

LOG="$(mktemp -t lake-build-bridge-XXXXXX.log)"
cleanup() { rm -f "$LOG"; }
trap cleanup EXIT

if lake build Bridge >"$LOG" 2>&1; then
    ok "lake build Bridge"
    if [ -s "$LOG" ]; then
        printf '%s\n' "--- build log ---"
        cat "$LOG"
        printf '%s\n' "--- end of log ---"
    fi
    RESULT=0
else
    ko "lake build Bridge"
    printf '%s\n' "--- build log (first errors) ---"
    head -n 80 "$LOG" || true
    printf '%s\n' "--- full log kept at: $LOG (trap disabled) ---"
    trap - EXIT
    RESULT=1
fi

# --- Summary ------------------------------------------------------------------

step "Summary"
printf 'PASS: %d  FAIL: %d  SKIP: %d\n' "$PASS" "$FAIL" "$SKIP"
if [ "$RESULT" -eq 0 ] && [ "$FAIL" -eq 0 ]; then
    echo "Lean toolchain ready and Bridge builds: the specification compiles."
    echo "Next: bash scripts/verify-local.sh (full local verification)."
    exit 0
else
    echo "Something failed - see the log above."
    exit 1
fi
