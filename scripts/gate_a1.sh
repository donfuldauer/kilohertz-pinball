#!/usr/bin/env bash
# Gate A1 runner (PLAN Gate A1): runs every criterion on the grey-box and
# prints a per-criterion PASS/FAIL table. Exits non-zero when anything fails.
#
#   OUT=target/gate_a1 GAMES=1000 SEED=1 scripts/gate_a1.sh
#
# OUT   output directory (default target/gate_a1): bundle/, logs/, soak_<skill>/
# GAMES soak games per skill (default 1000)
# SEED  soak seed (default 1)
# CARGO_TARGET_DIR and the other cargo variables come from the caller; the
# script only defaults CARGO_INCREMENTAL=0.
#
# Criteria and the test targets that cover them (a target whose file does not
# exist FAILS loudly with "missing test target <name>"; it is never skipped):
#   1a rolling 5/7 g sin(theta) within 1 %      pin_physics a1_analytic
#        (rolling_107m_*), ball_criteria (ball_rolls_down_an_incline_at_five_sevenths_g_sin)
#   1b drop-bounce height ratio = e^2 (2 %)     pin_physics a1_analytic
#        (drop_bounce_height_ratio_is_restitution_squared), ball_criteria (drop_bounce_*)
#   1c tilt bob period within 2 %               pin_physics mech_tilt
#        (tilt_bob_period_in_mech_set_follows_solver_gravity)
#   1d energy never increases, passive scenes   pin_app a1_bundle_passive
#   2  feel windows: stroke time, exit speed    pin_physics flipper_criteria
#        (unloaded_stroke_time_is_in_the_feel_window, ...), pin_app a1_bundle_feel
#   2b flipper in-lane / ramp behaviour         pin_app a1_inlane, a1_ramp
#   2c spec 2.3 criteria                        pin_physics spec_2_3_criteria
#   3  soak at novice/standard/expert: zero stuck/escaped/non-finite, 100 %
#      switch coverage, p99.9 tick < 0.35 ms at every skill; median ball
#      time 15..=120 s judged at `standard` only (the other medians are
#      reported). Judged by `pin_sim soak --gate a1`.
# The flipper dead-spot depth (<= 25 %) is REPORTED, not gating (DECISIONS
# "Orchestrator - Gate A1: the 25 % dead-spot depth is reported ..."); the
# lines come from spec_2_3_criteria and flipper_criteria with --nocapture.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
OUT="${OUT:-target/gate_a1}"
GAMES="${GAMES:-1000}"
SEED="${SEED:-1}"
LOGS="$OUT/logs"
mkdir -p "$LOGS"

NAMES=()
RESULTS=()
FAILED=0

record() { # name status
  NAMES+=("$1")
  RESULTS+=("$2")
  if [ "$2" != "PASS" ] && [ "$2" != "REPORT" ]; then FAILED=1; fi
}

has_target() { # package name
  [ -f "crates/$1/tests/$2.rs" ] || [ -f "crates/$1/tests/$2/main.rs" ]
}

run_test() { # criterion package target [extra cargo-test args after --]
  local crit="$1" pkg="$2" tgt="$3"
  shift 3
  if ! has_target "$pkg" "$tgt"; then
    echo "missing test target $tgt ($pkg) for: $crit"
    record "$crit [$pkg::$tgt]" "FAIL (missing test target $tgt)"
    return 0
  fi
  local log="$LOGS/${pkg}_${tgt}.log"
  echo "== cargo test -p $pkg --test $tgt"
  if cargo test -p "$pkg" --test "$tgt" -- "$@" >"$log" 2>&1; then
    record "$crit [$pkg::$tgt]" "PASS"
  else
    tail -n 25 "$log" || true
    record "$crit [$pkg::$tgt]" "FAIL (see $log)"
  fi
}

# A criterion with no covering test target at all prints NO TEST and fails.
no_test() { # criterion
  echo "NO TEST for $1"
  record "$1" "FAIL (NO TEST)"
}

echo "== building the grey-box bundle"
if tables/greybox/build.sh "$OUT/bundle" >"$LOGS/bundle.log" 2>&1; then
  record "grey-box bundle builds and validates" "PASS"
else
  tail -n 25 "$LOGS/bundle.log" || true
  record "grey-box bundle builds and validates" "FAIL (see $LOGS/bundle.log)"
fi

# 1. Analytic checks
run_test "1a rolling 5/7 g sin(theta) within 1 %" pin_physics a1_analytic
run_test "1a rolling 5/7 g sin(theta) (ball_criteria)" pin_physics ball_criteria
run_test "1b drop-bounce height ratio = e^2 within 2 %" pin_physics a1_analytic
run_test "1c tilt bob period within 2 %" pin_physics mech_tilt
run_test "1d energy never increases in passive scenes" pin_app a1_bundle_passive
# 2. Feel windows and flipper behaviour
run_test "2a flipper stroke time / exit speed windows" pin_physics flipper_criteria --nocapture
run_test "2b grey-box feel windows" pin_app a1_bundle_feel
run_test "2c spec 2.3 criteria" pin_physics spec_2_3_criteria --nocapture
run_test "2d in-lane ball leaves past the flipper" pin_app a1_inlane
run_test "2e ramp" pin_app a1_ramp

# Dead-spot depth: REPORT only.
for t in spec_2_3_criteria flipper_criteria; do
  log="$LOGS/pin_physics_${t}.log"
  if [ -f "$log" ]; then
    while IFS= read -r line; do
      echo "REPORT  dead-spot depth ($t): $line"
      record "dead-spot depth ($t): $line" "REPORT"
    done < <(grep -i "dead spot" "$log" || true)
  fi
done

# 3. Soak
echo "== cargo build --release -p pin_sim --bin pin_sim"
if cargo build --release -p pin_sim --bin pin_sim >"$LOGS/build_release.log" 2>&1; then
  PIN_SIM="${CARGO_TARGET_DIR:-target}/release/pin_sim"
  for skill in novice standard expert; do
    echo "== soak $skill: $GAMES games, seed $SEED"
    log="$LOGS/soak_${skill}.log"
    rc=0
    "$PIN_SIM" soak "$OUT/bundle" --games "$GAMES" --seed "$SEED" --skill "$skill" \
      --gate a1 --out "$OUT/soak_$skill" >"$log" 2>&1 || rc=$?
    grep -E '^(PASS|FAIL|REPORTED)  ' "$log" || true
    while IFS= read -r line; do
      status="${line%%  *}"
      rest="${line#*  }"
      case "$status" in
        PASS) record "3 soak $skill: $rest" "PASS" ;;
        REPORTED) record "3 soak $skill: $rest" "REPORT" ;;
        *) record "3 soak $skill: $rest" "FAIL" ;;
      esac
    done < <(grep -E '^(PASS|FAIL|REPORTED)  ' "$log" || true)
    if [ "$rc" -eq 0 ]; then
      record "3 soak $skill: exit status" "PASS"
    else
      tail -n 12 "$log" || true
      record "3 soak $skill: exit status" "FAIL (pin_sim exited $rc, see $log)"
    fi
  done
else
  tail -n 25 "$LOGS/build_release.log" || true
  record "3 release build of pin_sim" "FAIL (see $LOGS/build_release.log)"
fi

echo
echo "================ Gate A1 ================"
for i in "${!NAMES[@]}"; do
  res="${RESULTS[$i]}"
  word="${res%% *}"
  detail=""
  if [ "$word" != "$res" ]; then detail="  ${res#* }"; fi
  printf '%-7s %s%s\n' "$word" "${NAMES[$i]}" "$detail"
done
echo "========================================="
if [ "$FAILED" -eq 0 ]; then
  echo "GATE A1: PASS"
else
  echo "GATE A1: FAIL"
fi
exit "$FAILED"
