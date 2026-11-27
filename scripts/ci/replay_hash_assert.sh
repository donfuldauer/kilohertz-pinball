#!/usr/bin/env bash
# D9 reference-set gate (SPEC §0 row D9, §2.4; PROCESS R3): the replay-hash fixture's
# state_hash must equal the committed x86_64-linux golden on every desktop leg that runs this
# script (x86_64 Linux, aarch64 under qemu-user, macOS arm64, Windows x86_64, and x86_64 macOS
# under Rosetta 2 in release.yml). The one place the CI legs share this logic (KP-069, #163).
#
# Usage:
#   replay_hash_assert.sh <pin_sim> [<runner prefix>] [<golden>]
#       Record the fixture ($FIXTURE, default crates/pin_sim/tests/fixtures/replay_table) for
#       $TICKS ticks (default 10000), replay the recording 10 times, and fail unless every
#       replay equals the recording (same-build determinism) AND the recorded state_hash equals
#       the golden (default crates/pin_sim/tests/fixtures/replay_table.golden). <runner prefix>
#       is split on spaces and put before <pin_sim> (for example
#       "qemu-aarch64 -L /usr/aarch64-linux-gnu" or "/usr/bin/arch -x86_64"); "" for none.
#       $REPLAY_HASH_TARGET names the leg in annotations (default: uname -s/-m).
#   replay_hash_assert.sh --seam-digests <captured test output>
#       Check the `[seam digest]` lines that ball_seam_bitident prints with --nocapture: fail
#       unless exactly 5 were printed and every one says `same` (equal to the x86_64-linux
#       EXPECTED). Used where the test itself only prints (Windows x86_64) and, harmlessly, on
#       the legs where the test already asserts.
#
# Portable on purpose: bash 3.2 (macOS), Git Bash (Windows) and Linux bash; no associative
# arrays, no mapfile, no GNU-only flags. Carriage returns are stripped from the golden and from
# every program output, so a CRLF checkout on Windows compares equal.
set -euo pipefail

target="${REPLAY_HASH_TARGET:-$(uname -s)-$(uname -m)}"
title="replay-hash"

die() {
  echo "::error title=${title} ${target}::$1"
  exit 1
}

if [ "${1:-}" = "--seam-digests" ]; then
  title="seam digests"
  [ "$#" -eq 2 ] || die "usage: $0 --seam-digests <captured test output>"
  log="$2"
  [ -f "$log" ] || die "seam digest output $log not found"
  lines="$(tr -d '\r' < "$log" | grep '^\[seam digest\] ' || true)"
  if [ -z "$lines" ]; then
    total=0
  else
    total="$(printf '%s\n' "$lines" | wc -l | tr -d ' ')"
  fi
  same="$(printf '%s\n' "$lines" | grep -c ', same)$' || true)"
  if [ -n "$lines" ]; then
    printf '%s\n' "$lines"
  fi
  if printf '%s\n' "$lines" | grep -q 'differs'; then
    printf '%s\n' "$lines" | grep 'differs' | while IFS= read -r l; do
      echo "::error title=seam digests ${target}::${l}"
    done
    exit 1
  fi
  if [ "$total" -ne 5 ] || [ "$same" -ne 5 ]; then
    die "want 5 lines all 'same', got $total lines ($same same)"
  fi
  echo "::notice title=seam digests ${target}::5/5 equal the x86_64-linux EXPECTED"
  exit 0
fi

if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
  die "usage: $0 <pin_sim> [<runner prefix>] [<golden>]"
fi
exe="$1"
prefix="${2:-}"
golden="${3:-crates/pin_sim/tests/fixtures/replay_table.golden}"
fixture="${FIXTURE:-crates/pin_sim/tests/fixtures/replay_table}"
ticks="${TICKS:-10000}"

runner=()
if [ -n "$prefix" ]; then
  set -f
  # shellcheck disable=SC2206 # word splitting of the prefix is the point; globbing is off
  runner=($prefix)
  set +f
fi

[ -f "$golden" ] || die "golden $golden not found"
want="$(tr -d '\r' < "$golden")"
case "$want" in
  state_hash=????????????????) ;;
  *) die "golden $golden is not one 'state_hash=<16 hex digits>' line: '$want'" ;;
esac
if printf '%s' "${want#state_hash=}" | grep -q '[^0-9a-f]'; then
  die "golden $golden is not lowercase hex: '$want'"
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/replay-hash.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
rec="$tmp/replay_fixture.khzrec"

# Runs pin_sim (behind the runner prefix) and prints its state_hash= line, CR stripped.
hash_of() {
  local out
  out="$(${runner[@]+"${runner[@]}"} "$exe" "$@")" || return 1
  printf '%s\n' "$out" | tr -d '\r' | grep '^state_hash=' | head -n 1 || true
}

recorded="$(hash_of "$fixture" --ticks "$ticks" --record-inputs "$rec")" ||
  die "record run failed ($exe $fixture --ticks $ticks)"
[ -n "$recorded" ] || die "record run printed no state_hash= line"
echo "recorded:  $recorded ($target, $ticks ticks)"

fail=0
for i in 1 2 3 4 5 6 7 8 9 10; do
  if ! replayed="$(hash_of "$fixture" --replay-inputs "$rec")"; then
    echo "::error title=replay-hash ${target}::replay $i failed"
    fail=1
    continue
  fi
  echo "replay $i: $replayed"
  if [ "$replayed" != "$recorded" ]; then
    echo "::error title=replay-hash ${target}::replay $i gave '$replayed', recorded $recorded (in-machine mismatch)"
    fail=1
  fi
done

if [ "$recorded" != "$want" ]; then
  echo "::error title=replay-hash ${target}::recorded $recorded, committed x86_64-linux golden $want ($golden); D9 requires equality"
  fail=1
else
  echo "golden:    $want (equal)"
fi
echo "::notice title=replay-hash ${target}::${recorded#state_hash=}"
exit $fail
