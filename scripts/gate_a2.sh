#!/usr/bin/env bash
# Gate A2 runner (PLAN Gate A2, table soak). For every table it builds the
# bundle, soaks it at novice, standard and expert with `pin_sim soak`, runs the
# replay checks and the deferred Sky Deck readiness tests, and prints a
# per-criterion PASS/FAIL/REPORT/SKIPPED table. The verdict is computed from
# the artifacts by scripts/gate_a2_verdict.py and written to
# "$OUT/gate_a2_verdict.txt" (STATUS<TAB>table<TAB>criterion<TAB>value, then
# the final line) and "$OUT/gate_a2_table.txt" before the final line is
# printed. Exit 0 only for "GATE A2: PASS" (or a dry run without FAIL).
#
#   scripts/gate_a2.sh                       full gate (3 tables x 3 skills x 1000 games)
#   DRY=1 TABLES="apex_circuit" scripts/gate_a2.sh   smoke run
#   scripts/gate_a2.sh --verdict-only OUT    recompute the verdict from OUT's artifacts
#
# OUT        output directory (default target/gate_a2; a default that resolves
#            outside the repository's target/ is refused)
# GAMES      games per skill (default 1000; DRY: 20)
# SEED       soak seed (default 1)
# THREADS    soak worker threads (default 2)
# TABLES     space-separated table directory names (default: every tables/*/
#            with a build.sh except greybox; greybox runs only when named)
# REPLAYS    sampled replays per table (default 20; DRY: 3)
# PATTERN_TICKS  ticks of each recorded built-in input pattern (default 60000)
# DRY=1      dry run: smaller defaults, the 1,000-game and 20-replay checks are
#            REPORTed, the fixed-size readiness soaks are SKIPPED with a reason,
#            and the final line is "GATE A2 (DRY RUN): ..."
# READINESS=0  skip the Sky Deck readiness tests (DRY only)
# Cargo: CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 and line-tables-only debug info
# are defaulted; CARGO_TARGET_DIR comes from the caller.
#
# Only a run with GAMES>=1000, REPLAYS>=20 and no DRY can print "GATE A2: PASS".
# Decisions: docs/decisions/2026-11-19-gate-a2-runner.md.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
REPO="$(pwd -P)"
VERDICT_PY="$REPO/scripts/gate_a2_verdict.py"

need_python() {
  if ! command -v python3 >/dev/null 2>&1; then
    echo "gate_a2.sh: the verdict needs python3 (run as python3 -I); none found" >&2
    return 1
  fi
}

if [ "${1:-}" = "--verdict-only" ]; then
  if [ "$#" -ne 2 ] || [ ! -d "$2" ]; then
    echo "usage: scripts/gate_a2.sh --verdict-only OUT  (OUT: an existing gate_a2 output directory)" >&2
    exit 2
  fi
  need_python || exit 1
  exec python3 -I "$VERDICT_PY" "$2"
fi
if [ "$#" -ne 0 ]; then
  echo "usage: [env vars] scripts/gate_a2.sh | scripts/gate_a2.sh --verdict-only OUT" >&2
  exit 2
fi

DRY="${DRY:-0}"
if [ "$DRY" = "1" ]; then
  GAMES="${GAMES:-20}"
  REPLAYS="${REPLAYS:-3}"
else
  DRY=0
  GAMES="${GAMES:-1000}"
  REPLAYS="${REPLAYS:-20}"
fi
SEED="${SEED:-1}"
THREADS="${THREADS:-2}"
PATTERN_TICKS="${PATTERN_TICKS:-60000}"
READINESS="${READINESS:-1}"
for v in GAMES REPLAYS SEED THREADS PATTERN_TICKS; do
  case "${!v}" in
    '' | *[!0-9]*) echo "gate_a2.sh: $v must be a non-negative integer, got '${!v}'" >&2; exit 2 ;;
  esac
done
if [ "$GAMES" -lt 1 ] || [ "$THREADS" -lt 1 ] || [ "$REPLAYS" -lt 1 ]; then
  echo "gate_a2.sh: GAMES, THREADS and REPLAYS must be at least 1" >&2
  exit 2
fi
if [ "$REPLAYS" -gt "$GAMES" ]; then
  echo "gate_a2.sh: REPLAYS ($REPLAYS) cannot exceed GAMES ($GAMES)" >&2
  exit 2
fi
if [ "$READINESS" = "0" ] && [ "$DRY" != "1" ]; then
  echo "gate_a2.sh: READINESS=0 is only allowed with DRY=1" >&2
  exit 2
fi
need_python || exit 1

if [ -n "${OUT+x}" ] && [ -n "$OUT" ]; then
  OUT_GIVEN=1
else
  OUT_GIVEN=0
  OUT="target/gate_a2"
fi
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd -P)"
if [ "$OUT_GIVEN" -eq 0 ]; then
  case "$OUT/" in
    "$REPO/target/"*) ;;
    *) echo "gate_a2.sh: default OUT resolves to $OUT, outside $REPO/target/; set OUT explicitly" >&2; exit 2 ;;
  esac
fi

echo "== disk space for $OUT"
df -h "$OUT"
avail_kb="$(df -Pk "$OUT" | awk 'NR == 2 { print $4 }')"
if [ -z "$avail_kb" ] || [ "$avail_kb" -lt $((4 * 1024 * 1024)) ]; then
  echo "gate_a2.sh: less than 4 GB free on the filesystem of $OUT (${avail_kb:-?} KB); aborting" >&2
  exit 1
fi

if [ -z "${TABLES:-}" ]; then
  TABLES=""
  for d in tables/*/; do
    t="$(basename "$d")"
    if [ -f "tables/$t/build.sh" ] && [ "$t" != "greybox" ]; then TABLES="$TABLES $t"; fi
  done
fi
# Fixed order (D9): sorted, unique.
# shellcheck disable=SC2086 # word splitting of the table list is intended
TABLES="$(printf '%s\n' $TABLES | LC_ALL=C sort -u | tr '\n' ' ')"
TABLES="${TABLES% }"
for t in $TABLES; do
  case "$t" in
    *[!A-Za-z0-9_-]*) echo "gate_a2.sh: bad table name '$t'" >&2; exit 2 ;;
  esac
done
if [ -z "$TABLES" ]; then
  echo "gate_a2.sh: no tables to run" >&2
  exit 2
fi
has_table() { case " $TABLES " in *" $1 "*) return 0 ;; esac; return 1; }

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-line-tables-only}"

if [ "$THREADS" -gt 1 ]; then REPLAY_THREADS=1; else REPLAY_THREADS=2; fi

LOGS="$OUT/logs"
mkdir -p "$LOGS"
# Stale artifacts of an earlier run must not be judged: remove this run's
# status files (only files, only under OUT).
rm -f "$OUT/gate_a2_verdict.txt" "$OUT/gate_a2_table.txt" "$OUT/readiness.tsv" "$OUT/gate_a2_run.txt"
for t in $TABLES; do
  rm -f "$OUT/$t/build.rc" "$OUT/$t/replay_soak.rc" "$OUT/$t/replay_pattern.tsv" \
    "$OUT/$t/replay_soak/soak_report.json" "$OUT/$t/replay_soak/soak_report.md"
  for skill in novice standard expert; do
    rm -f "$OUT/$t/soak_$skill.rc" "$OUT/$t/soak_$skill/soak_report.json" "$OUT/$t/soak_$skill/soak_report.md"
  done
done
{
  echo "dry=$DRY"
  echo "games=$GAMES"
  echo "replays=$REPLAYS"
  echo "seed=$SEED"
  echo "threads=$THREADS"
  echo "replay_threads=$REPLAY_THREADS"
  echo "pattern_ticks=$PATTERN_TICKS"
  echo "readiness=$READINESS"
  echo "tables=$TABLES"
} >"$OUT/gate_a2_run.txt"
echo "== Gate A2: TABLES='$TABLES' GAMES=$GAMES REPLAYS=$REPLAYS SEED=$SEED THREADS=$THREADS DRY=$DRY OUT=$OUT"

now() { date +%s; }

echo "== cargo build --release -p pin_sim --bin pin_sim"
PIN_SIM=""
if cargo build --release -p pin_sim --bin pin_sim --message-format=json-render-diagnostics \
  >"$LOGS/build_release.json" 2>"$LOGS/build_release.log"; then
  if command -v jq >/dev/null 2>&1; then
    PIN_SIM="$(jq -r 'select(.reason == "compiler-artifact" and .target.name == "pin_sim"
      and (.target.kind | index("bin")) and .executable != null) | .executable' \
      "$LOGS/build_release.json" | tail -n 1)"
  else
    PIN_SIM="$(python3 -I -c '
import json, sys
exe = ""
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if (m.get("reason") == "compiler-artifact" and m["target"]["name"] == "pin_sim"
            and "bin" in m["target"]["kind"] and m.get("executable")):
        exe = m["executable"]
print(exe)' "$LOGS/build_release.json")"
  fi
fi
if [ -z "$PIN_SIM" ] || [ ! -x "$PIN_SIM" ]; then
  tail -n 25 "$LOGS/build_release.log" || true
  echo "gate_a2.sh: no pin_sim release binary (see $LOGS/build_release.log); every soak criterion FAILs"
  PIN_SIM=""
fi

for t in $TABLES; do
  tdir="$OUT/$t"
  tlogs="$tdir/logs"
  mkdir -p "$tlogs"
  echo "== [$t] building the bundle"
  rc=0
  if [ -f "tables/$t/build.sh" ]; then
    "tables/$t/build.sh" "$tdir/bundle" >"$tlogs/build.log" 2>&1 || rc=$?
  else
    echo "no tables/$t/build.sh" >"$tlogs/build.log"
    rc=127
  fi
  echo "$rc" >"$tdir/build.rc"
  if [ "$rc" -ne 0 ]; then
    tail -n 25 "$tlogs/build.log" || true
    echo "== [$t] bundle build failed (exit $rc); its soak criteria FAIL"
    continue
  fi
  if [ -z "$PIN_SIM" ]; then continue; fi

  for skill in novice standard expert; do
    echo "== [$t] soak $skill: $GAMES games, seed $SEED, $THREADS threads"
    log="$tlogs/soak_$skill.log"
    t0="$(now)"
    rc=0
    "$PIN_SIM" soak "$tdir/bundle" --games "$GAMES" --seed "$SEED" --skill "$skill" \
      --threads "$THREADS" --out "$tdir/soak_$skill" >"$log" 2>&1 || rc=$?
    echo "$rc" >"$tdir/soak_$skill.rc"
    grep -E '^soak ' "$log" || tail -n 3 "$log" || true
    echo "   [$t] soak $skill: exit $rc, $(($(now) - t0)) s wall"
  done

  echo "== [$t] A2.4a: standard soak re-run for $REPLAYS games with $REPLAY_THREADS threads"
  rc=0
  "$PIN_SIM" soak "$tdir/bundle" --games "$REPLAYS" --seed "$SEED" --skill standard \
    --threads "$REPLAY_THREADS" --out "$tdir/replay_soak" >"$tlogs/replay_soak.log" 2>&1 || rc=$?
  echo "$rc" >"$tdir/replay_soak.rc"

  echo "== [$t] A2.4b: $REPLAYS built-in pattern recordings of $PATTERN_TICKS ticks, replayed"
  mkdir -p "$tdir/replay_pattern"
  : >"$tdir/replay_pattern.tsv"
  plog="$tlogs/replay_pattern.log"
  : >"$plog"
  i=0
  while [ "$i" -lt "$REPLAYS" ]; do
    s=$((SEED + i))
    rec="$tdir/replay_pattern/seed_$s.khzrec"
    recorded="$("$PIN_SIM" "$tdir/bundle" --ticks "$PATTERN_TICKS" --seed "$s" --record-inputs "$rec" 2>>"$plog" || true)"
    replayed="$("$PIN_SIM" "$tdir/bundle" --replay-inputs "$rec" 2>>"$plog" || true)"
    rh="$(printf '%s\n' "$recorded" | sed -n 's/^state_hash=\([0-9a-f]*\)$/\1/p' | tail -n 1)"
    ph="$(printf '%s\n' "$replayed" | sed -n 's/^state_hash=\([0-9a-f]*\)$/\1/p' | tail -n 1)"
    echo "seed $s: recorded '$recorded' replayed '$replayed'" >>"$plog"
    printf '%s\t%s\t%s\n' "$s" "$rh" "$ph" >>"$tdir/replay_pattern.tsv"
    i=$((i + 1))
  done
done

# Deferred readiness tests (Sky Deck, PR #60): key target test ignored?
has_target() { # package name
  [ -f "crates/$1/tests/$2.rs" ] || [ -f "crates/$1/tests/$2/main.rs" ]
}
readiness() { # key target test ignored(0|1)
  local key="$1" tgt="$2" name="$3" ign="$4" log="$LOGS/readiness_$1.log" rc=0
  if ! has_target pin_app "$tgt"; then
    printf '%s\tMISSING\tmissing test target %s (pin_app)\n' "$key" "$tgt" >>"$OUT/readiness.tsv"
    echo "== readiness $key: missing test target $tgt"
    return 0
  fi
  if [ "$ign" = "1" ] && [ "$DRY" = "1" ]; then
    printf '%s\tSKIPPED\tdry run: %s has a fixed game count, its minimum is the full test; run without DRY\n' \
      "$key" "$name" >>"$OUT/readiness.tsv"
    echo "== readiness $key: SKIPPED in dry run ($name has a fixed game count)"
    return 0
  fi
  local args=(--exact --nocapture "$name")
  if [ "$ign" = "1" ]; then args=(--ignored "${args[@]}"); fi
  echo "== readiness $key: cargo test --release -p pin_app --test $tgt -- ${args[*]}"
  local t0
  t0="$(now)"
  cargo test --release -p pin_app --test "$tgt" -- "${args[@]}" >"$log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && grep -q 'test result: ok. 1 passed' "$log"; then
    printf '%s\tPASS\t1 passed, %s s wall (see %s)\n' "$key" "$(($(now) - t0))" "$log" >>"$OUT/readiness.tsv"
  else
    tail -n 25 "$log" || true
    printf '%s\tFAIL\texit %s, not exactly 1 test passed (see %s)\n' "$key" "$rc" "$log" >>"$OUT/readiness.tsv"
  fi
}
: >"$OUT/readiness.tsv"
if has_table sky_deck; then
  if [ "$READINESS" = "0" ]; then
    for key in r6c c6a c6c_recorded c6c_sampled; do
      printf '%s\tSKIPPED\tREADINESS=0\n' "$key" >>"$OUT/readiness.tsv"
    done
  else
    readiness r6c sky_deck_soak r6c_ball_times_200_games_per_skill 1
    readiness c6a spec_6_2_soak c6a_soak_readiness_40_games_per_skill 1
    readiness c6c_recorded spec_6_2_soak c6c_recorded_inputs_replay_identically_10_times 0
    readiness c6c_sampled spec_6_2_soak c6c_sampled_bot_games_replay_identically 0
  fi
else
  echo "== readiness: SKIPPED (sky_deck not in TABLES)"
fi

echo
exec python3 -I "$VERDICT_PY" "$OUT"
