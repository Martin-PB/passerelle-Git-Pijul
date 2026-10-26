#!/usr/bin/env bash
# Local verification for Passerelle Git-Pijul.
# Does not install packages, modify source files, push refs, or trigger GitHub Actions.
set -Eeuo pipefail
IFS=$'\n\t'

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_DIR="${LOG_DIR:-$ROOT/.local-verification/$STAMP}"
mkdir -p "$LOG_DIR"
SUMMARY="$LOG_DIR/summary.txt"
: > "$SUMMARY"

PASS=0
FAIL=0
SKIP=0
WARN=0

say() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
has() { command -v "$1" >/dev/null 2>&1; }

run_check() {
  local name="$1"; shift
  local logfile="$LOG_DIR/$name.log"
  say ""
  say "===== $name ====="
  say "\$ $*"
  if "$@" > >(tee "$logfile") 2>&1; then
    say "RESULT: PASS ($name)"
    PASS=$((PASS + 1))
  else
    local rc=$?
    say "RESULT: FAIL ($name, exit $rc)"
    FAIL=$((FAIL + 1))
  fi
}

skip_check() {
  say "RESULT: SKIP ($1) — $2"
  SKIP=$((SKIP + 1))
}

warn() {
  say "WARNING: $*"
  WARN=$((WARN + 1))
}

say "Passerelle Git-Pijul — local verification"
say "UTC start: $STAMP"
say "Repository: $ROOT"
say "Branch/commit: $(git -C "$ROOT" branch --show-current 2>/dev/null || echo unknown) / $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
say "Logs: $LOG_DIR"
say "No GitHub Actions will be triggered."

# Mandatory host tools
missing=()
for bin in git cargo rustc; do
  has "$bin" || missing+=("$bin")
done
if (("${#missing[@]}" > 0)); then
  say "FATAL: missing required command(s): ${missing[*]}"
  say "Install Rust/Cargo and Git using your Linux distribution's trusted package manager or rustup, then rerun."
  exit 2
fi

say ""
say "===== Tool versions ====="
git --version | tee -a "$SUMMARY"
rustc --version | tee -a "$SUMMARY"
cargo --version | tee -a "$SUMMARY"

if has pijul; then
  pijul --version 2>&1 | tee "$LOG_DIR/pijul-version.log" | tee -a "$SUMMARY"
  pijul --version 2>&1 | grep -Fq '1.0.0-beta.24' || warn "Pijul n'est pas identifié comme 1.0.0-beta.24. Les tests d'intégration doivent être interprétés avec prudence ; ne pas substituer silencieusement une autre version."
else
  warn "pijul absent : les tests qui exigent le vrai binaire Pijul peuvent être ignorés ou échouer. Aucun test end-to-end ne pourra être considéré validé."
fi

# 1. Rust formatting and static checks
run_check rustfmt-check cargo fmt --all -- --check
if [[ ! -f Cargo.lock ]]; then warn "Cargo.lock absent : Cargo résoudra les dépendances à partir des contraintes de Cargo.toml ; la reproductibilité complète exige de versionner Cargo.lock."; fi
run_check cargo-test-all-targets cargo test --all-targets

# 2. Optional Pijul integration tests. Git-only corpus tests run in the default
# suite; tests that need the Pijul binary remain ignored unless explicitly selected.
if [[ "${RUN_PIJUL_IMPORT_TESTS:-0}" == "1" ]]; then
  if ! has pijul; then
    say "RESULT: FAIL pijul-import-triage — RUN_PIJUL_IMPORT_TESTS=1 mais pijul est absent."
    FAIL=$((FAIL + 1))
  else
    PIJUL_VERSION="$(pijul --version 2>&1 || true)"
    say "Pijul version: $PIJUL_VERSION"
    if [[ "$PIJUL_VERSION" != *"1.0.0-beta.24"* ]]; then
      say "RESULT: FAIL pijul-import-triage — ce test exige Pijul 1.0.0-beta.24."
      FAIL=$((FAIL + 1))
    else
      run_check pijul-import-triage cargo test --test roundtrip synthetic_import_pipeline_exposes_native_pijul_conflicts -- --ignored --nocapture
      run_check sibling-branch-rerun cargo test --test roundtrip sibling_branches_then_rerun_preserves_base_and_mapping -- --ignored --nocapture
    fi
  fi
else
  skip_check pijul-import-tests "opt-in : lancer avec RUN_PIJUL_IMPORT_TESTS=1 pour exécuter l'import/triage Pijul et le scénario branches sœurs + rerun ; le corpus Git tourne dans cargo test --all-targets."
fi

# 3. Lean formalization. Lake uses the repository's lean-toolchain pin.
if has lake; then
  run_check lean-lake-build lake build Bridge
else
  skip_check lean-lake-build "lake absent. Installer elan/Lean 4 puis relancer ; les fichiers Lean ne sont pas considérés comme vérifiés."
  warn "Aucune preuve Lean vérifiée durant cette exécution."
fi

# 4. Optional end-to-end round-trip. This remains a separate, known-incomplete
# target because the corpus currently requires explicit resolution changes.
if [[ "${RUN_IGNORED_ROUNDTRIP:-0}" == "1" ]]; then
  if ! has pijul || ! has pijul-export; then
    say "RESULT: FAIL ignored-roundtrip — RUN_IGNORED_ROUNDTRIP=1 exige pijul et pijul-export."
    FAIL=$((FAIL + 1))
  else
    run_check ignored-roundtrip cargo test --test roundtrip roundtrip_property_end_to_end -- --ignored --nocapture
  fi
else
  skip_check ignored-roundtrip "opt-in : lancer avec RUN_IGNORED_ROUNDTRIP=1 ; ce test reste incomplet tant que les résolutions du corpus ne sont pas déterministes."
fi

# 5. Backward-compatible opt-in for only the sibling-branch regression.
if [[ "${RUN_SIBLING_RERUN_TEST:-0}" == "1" && "${RUN_PIJUL_IMPORT_TESTS:-0}" != "1" ]]; then
  if ! has pijul; then
    say "RESULT: FAIL sibling-branch-rerun — RUN_SIBLING_RERUN_TEST=1 mais pijul est absent."
    FAIL=$((FAIL + 1))
  else
    run_check sibling-branch-rerun cargo test --test roundtrip sibling_branches_then_rerun_preserves_base_and_mapping -- --ignored --nocapture
  fi
else
  skip_check sibling-branch-rerun "opt-in : lancer avec RUN_SIBLING_RERUN_TEST=1 ; déjà exécuté par RUN_PIJUL_IMPORT_TESTS=1 si ce mode est actif."
fi

say ""
say "===== FINAL SUMMARY ====="
say "PASS=$PASS FAIL=$FAIL SKIP=$SKIP WARN=$WARN"
say "Detailed logs: $LOG_DIR"
if (( FAIL > 0 )); then
  say "OVERALL: FAIL — consulter les journaux ; aucun échec n'est masqué."
  exit 1
fi
if (( SKIP > 0 || WARN > 0 )); then
  say "OVERALL: INCOMPLETE — les étapes ignorées/avertissements ne sont pas des validations."
  # Return non-zero so automation cannot mistake a partial run for full validation.
  exit 2
fi
say "OVERALL: PASS — toutes les étapes configurées ont réussi."
