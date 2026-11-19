#!/usr/bin/env python3
"""T1: Gate A2 verdict tests on hand-made soak_report.json files.

Run: ``python3 -I scripts/tests/gate_a2_verdict_test.py`` (no cargo, no GPU,
no simulation; each case builds a temp OUT and runs
``scripts/gate_a2.sh --verdict-only OUT``).
"""

import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
GATE = os.path.join(REPO, "scripts", "gate_a2.sh")
SKILLS = ("novice", "standard", "expert")
TABLES = ("apex_circuit", "feature_lab", "sky_deck")
SCORES = {"novice": 1000, "standard": 2000, "expert": 3000}
READY = ("r6c", "c6a", "c6c_recorded", "c6c_sampled")


def report(skill, games=1000, ticks=120_000):
    """A passing schema-1 report in SoakReport::to_json's shape."""
    return {
        "schema": 1,
        "table": {"id": "t", "hash": "0000000000000001"},
        "config": {"games": games, "seed": 1, "skill": skill, "max_game_ticks": 1_800_000},
        "summary": {"passed": True, "coverage_complete": True, "games": games},
        "findings": [],
        "coverage": {
            "never_hit": [],
            "switches": [{"name": "sw_a", "hits": 5}, {"name": "sw_b", "hits": 7}],
            "shots": [{"id": "left_orbit", "count": 4}, {"id": "drop_bank", "count": 9},
                      {"id": "ramp", "count": 12}, {"id": "scoop", "count": 30}],
            "modes": [{"id": "multiball", "starts": 3, "observable": True}],
        },
        "distributions": {
            "ball_time_ms": {"count": 3, "min": 1, "median": 30000, "p90": 1, "p99": 1, "max": 1},
            "ball_time_censored": 0,
            "game_score": {"count": games, "min": 0, "median": SCORES[skill], "p90": 0, "p99": 0, "max": 0},
        },
        "counters": {"edges_dropped": 0, "haptics_dropped": 0, "inputs_dropped": 0, "spawns_failed": 0,
                     "ball_events_dropped": 0, "balls_drained": 3, "balls_escaped": 0,
                     "script_queue_overflows": 0, "script_faults": 0},
        "games": [{"game": i, "seed": 1000 + i, "ticks": ticks, "end": "game_over",
                   "state_hash": "%016x" % (0xABC000 + i), "score": SCORES[skill], "balls": 3}
                  for i in range(games)],
        "timing": {"threads": 2, "wall_s": 10.0},
    }


def spread(games, n):
    return [i * games // n for i in range(n)]


def replay_rows(rep, replays):
    """replay_games.tsv rows consistent with a standard report: the spread
    games, each replayed to the soak's own state_hash and score."""
    games = rep["config"]["games"]
    by = {g["game"]: g for g in rep["games"]}
    rows = []
    for g in spread(games, replays):
        e = by[g]
        sc = "none" if e["score"] is None else str(e["score"])
        rows.append([str(g), e["state_hash"], e["state_hash"], sc, sc, str(e["ticks"])])
    return rows


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


class Out:
    """A synthetic gate output directory that passes in full."""

    def __init__(self, tables=TABLES, games=1000, replays=20, dry=0):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name
        self.tables = list(tables)
        self.replays = replays
        write(os.path.join(self.dir, "gate_a2_run.txt"),
              "dry=%d\ngames=%d\nreplays=%d\nseed=1\nthreads=2\nreadiness=1\ntables=%s\n"
              % (dry, games, replays, " ".join(tables)))
        self.reports = {}
        # A2.4c: per table, replay_games.tsv rows (lists of 6 strings);
        # None means "do not write the file".
        self.replay_games = {}
        for t in tables:
            write(os.path.join(self.dir, t, "build.rc"), "0\n")
            for s in SKILLS:
                self.reports[(t, s)] = report(s, games)
                write(os.path.join(self.dir, t, "soak_%s.rc" % s), "0\n")
            rep = report("standard", replays)
            rep["timing"]["threads"] = 1
            self.reports[(t, "replay")] = rep
            self.replay_games[t] = replay_rows(self.reports[(t, "standard")], replays)
            write(os.path.join(self.dir, t, "replay_soak.rc"), "0\n")
            write(os.path.join(self.dir, t, "replay_pattern.tsv"),
                  "".join("%d\t%016x\t%016x\n" % (i, i * 77, i * 77) for i in range(1, replays + 1)))
        write(os.path.join(self.dir, "readiness.tsv"),
              "".join("%s\tPASS\t1 passed\n" % k for k in READY))

    def flush(self):
        for (t, s), r in self.reports.items():
            sub = "replay_soak" if s == "replay" else "soak_%s" % s
            write(os.path.join(self.dir, t, sub, "soak_report.json"), json.dumps(r, indent=1))
        for t, rows in self.replay_games.items():
            path = os.path.join(self.dir, t, "replay_games.tsv")
            if rows is None:
                if os.path.exists(path):
                    os.remove(path)
            else:
                write(path, "".join("\t".join(r) + "\n" for r in rows))

    def run(self):
        self.flush()
        p = subprocess.run(["bash", GATE, "--verdict-only", self.dir], capture_output=True, text=True)
        lines = p.stdout.rstrip("\n").split("\n")
        rows = []
        with open(os.path.join(self.dir, "gate_a2_verdict.txt"), encoding="utf-8") as f:
            vlines = f.read().rstrip("\n").split("\n")
        for line in vlines[:-1]:
            rows.append(tuple(line.split("\t")))
        return p.returncode, lines, vlines, rows

    def close(self):
        self.tmp.cleanup()


def status_of(rows, table, crit_prefix):
    got = [r[0] for r in rows if r[1] == table and r[2].startswith(crit_prefix)]
    assert got, "no row %s / %s in %r" % (table, crit_prefix, [r[2] for r in rows if r[1] == table])
    return got


class VerdictTest(unittest.TestCase):
    def setUp(self):
        self.out = Out()

    def tearDown(self):
        self.out.close()

    def check_fail(self, table, crit_prefix):
        rc, lines, vlines, rows = self.out.run()
        self.assertEqual(rc, 1, lines[-1])
        self.assertIn("FAIL", status_of(rows, table, crit_prefix))
        self.assertTrue(lines[-1].startswith("GATE A2: FAIL ("), lines[-1])
        self.assertEqual(lines[-1], vlines[-1])
        # Exactly the failing criterion fails.
        fails = [r for r in rows if r[0] == "FAIL"]
        self.assertTrue(all(r[1] == table and r[2].startswith(crit_prefix) for r in fails), fails)
        return rows

    # (a) all pass
    def test_all_pass(self):
        rc, lines, vlines, rows = self.out.run()
        self.assertEqual(lines[-1], "GATE A2: PASS", "\n".join(lines))
        self.assertEqual(rc, 0)
        self.assertEqual([r for r in rows if r[0] == "FAIL"], [])
        for t in TABLES:
            for crit in ("A2.0 games", "A2.1a", "A2.1b", "A2.1c", "A2.2a", "A2.2b", "A2.2c",
                         "A2.3a", "A2.3b", "A2.4 sampled", "A2.4a (supplemental)",
                         "A2.4b (supplemental)"):
                self.assertIn("PASS", status_of(rows, t, crit))
            self.assertEqual(status_of(rows, t, "A2.4c"), ["PASS"])
            c = [r for r in rows if r[1] == t and r[2].startswith("A2.4c")][0]
            self.assertEqual(c[3], "20 of 20 identical")
        # The old non-gating bot-replay REPORT row is gone.
        self.assertFalse(any("repeatability check used" in r[3] for r in rows))
        self.assertEqual(len([r for r in rows if r[1] == "readiness" and r[0] == "PASS"]), 4)
        # Context REPORT lines.
        apex = [r for r in rows if r[1] == "apex_circuit" and r[0] == "REPORT"]
        self.assertTrue(any("Firewall Save, Apex Jackpot" in r[3] for r in apex))
        self.assertTrue(any(r[2].startswith("A2.2b drop_bank count") and "over-credited" in r[3] for r in apex))
        thin = [r for r in apex if r[2].startswith("A2.2b thinnest")]
        self.assertEqual(thin[0][3], "left_orbit=4, drop_bank=9, ramp=12")
        self.assertFalse(any("Firewall" in r[3] for r in rows if r[1] == "feature_lab"))

    def test_pass_needs_replay_data(self):
        os.remove(os.path.join(self.out.dir, "feature_lab", "replay_pattern.tsv"))
        self.check_fail("feature_lab", "A2.4b")

    def test_pass_needs_replay_soak(self):
        del self.out.reports[("sky_deck", "replay")]
        self.check_fail("sky_deck", "A2.4a")

    def test_replay_soak_mismatch(self):
        self.out.reports[("feature_lab", "replay")]["games"][7]["state_hash"] = "deadbeefdeadbeef"
        rows = self.check_fail("feature_lab", "A2.4a")
        self.assertTrue(any("game 7" in r[3] for r in rows if r[0] == "FAIL"))

    def test_pattern_mismatch(self):
        write(os.path.join(self.out.dir, "apex_circuit", "replay_pattern.tsv"),
              "".join("%d\t%016x\t%016x\n" % (i, 1, 1 if i != 5 else 2) for i in range(1, 21)))
        self.check_fail("apex_circuit", "A2.4b")

    def test_replays_19(self):
        self.out.close()
        self.out = Out(replays=19)
        rc, lines, _, rows = self.out.run()
        self.assertEqual(rc, 1)
        self.assertTrue(lines[-1].startswith("GATE A2: FAIL"), lines[-1])
        fails = sorted((r[1], r[2][:5] if r[2].startswith("A2.4c") else r[2]) for r in rows if r[0] == "FAIL")
        self.assertEqual(fails, sorted([(t, "A2.4 sampled replays >= 20") for t in TABLES]
                                       + [(t, "A2.4c") for t in TABLES]))
        for r in rows:
            if r[0] == "FAIL" and r[2].startswith("A2.4c"):
                self.assertIn("REPLAYS=19 < 20", r[3])

    # A2.4c: sampled bot games replayed from recorded input (finding 1)
    def check_a24c(self, table, needle):
        rows = self.check_fail(table, "A2.4c")
        val = [r[3] for r in rows if r[0] == "FAIL"][0]
        self.assertIn(needle, val)
        return val

    def test_a24c_file_missing(self):
        self.out.replay_games["feature_lab"] = None
        self.check_a24c("feature_lab", "no ")

    def test_a24c_fewer_rows(self):
        del self.out.replay_games["apex_circuit"][-1]
        self.check_a24c("apex_circuit", "19 rows")

    def test_a24c_first_games_not_spread(self):
        rep = self.out.reports[("sky_deck", "standard")]
        rows = replay_rows(rep, 1000)[:20]  # games 0..19, not the spread
        self.out.replay_games["sky_deck"] = rows
        val = self.check_a24c("sky_deck", "expected 20 (spread")
        self.assertIn("missing 50,100,150", val)

    def test_a24c_replay_hash_differs(self):
        self.out.replay_games["feature_lab"][4][2] = "deadbeefdeadbeef"
        val = self.check_a24c("feature_lab", "game 200: replay state_hash deadbeefdeadbeef")
        self.assertTrue(val.startswith("19 of 20 identical"), val)

    def test_a24c_empty_replay_hash(self):
        self.out.replay_games["apex_circuit"][0][2] = ""
        self.check_a24c("apex_circuit", "game 0: no replay state_hash")

    def test_a24c_empty_replay_score(self):
        self.out.replay_games["apex_circuit"][3][4] = ""
        self.check_a24c("apex_circuit", "game 150: no replay score")

    def test_a24c_score_differs(self):
        self.out.replay_games["sky_deck"][19][4] = "1"
        self.check_a24c("sky_deck", "game 950: replay score 1 != soak 2000")

    def test_a24c_soak_hash_not_the_report(self):
        # Replay equals the tsv's soak value, but that is not the report's.
        row = self.out.replay_games["feature_lab"][7]
        row[1] = row[2] = "0123456789abcdef"
        self.check_a24c("feature_lab", "game 350: recorded soak state_hash")

    def test_a24c_soak_score_not_the_report(self):
        row = self.out.replay_games["feature_lab"][2]
        row[3] = row[4] = "7"
        self.check_a24c("feature_lab", "game 100: recorded soak score")

    def test_a24c_game_missing_from_report(self):
        r = self.out.reports[("apex_circuit", "standard")]
        r["games"] = [g for g in r["games"] if g["game"] != 500]
        rc, lines, _, rows = self.out.run()
        self.assertEqual(rc, 1)
        self.assertIn("FAIL", status_of(rows, "apex_circuit", "A2.4c"))
        val = [x[3] for x in rows if x[1] == "apex_circuit" and x[2].startswith("A2.4c")][0]
        self.assertIn("game 500: not in the standard soak report", val)

    def test_a24c_none_score(self):
        r = self.out.reports[("apex_circuit", "standard")]
        for g in r["games"]:
            if g["game"] == 50:
                g["score"] = None
        self.out.replay_games["apex_circuit"] = replay_rows(r, 20)
        self.assertEqual(self.out.replay_games["apex_circuit"][1][3], "none")
        rc, lines, _, rows = self.out.run()
        self.assertEqual((rc, lines[-1]), (0, "GATE A2: PASS"))

    def test_a24c_unparseable_line(self):
        with_bad = self.out.replay_games["sky_deck"]
        with_bad.append(["x", "", "", "", "", ""])
        self.check_a24c("sky_deck", "line 21 unparseable")

    def test_a24c_missing_file_fails_in_dry_run_with_feature(self):
        o = Out(games=12, replays=3, dry=1)
        try:
            o.replay_games["apex_circuit"] = None
            rc, lines, _, rows = o.run()
            self.assertEqual(rc, 1)
            self.assertEqual(status_of(rows, "apex_circuit", "A2.4c"), ["FAIL"])
        finally:
            o.close()

    def test_a24c_missing_file_reported_in_dry_run_without_feature(self):
        o = Out(games=12, replays=3, dry=1)
        try:
            with open(os.path.join(o.dir, "gate_a2_run.txt"), "a", encoding="utf-8") as f:
                f.write("record_games=0\n")
            for t in TABLES:
                o.replay_games[t] = None
            rc, lines, _, rows = o.run()
            self.assertEqual(rc, 0, lines[-1])
            self.assertEqual(status_of(rows, "apex_circuit", "A2.4c"), ["REPORT"])
        finally:
            o.close()

    def test_a24c_missing_file_fails_in_full_run_without_feature(self):
        with open(os.path.join(self.out.dir, "gate_a2_run.txt"), "a", encoding="utf-8") as f:
            f.write("record_games=0\n")
        self.out.replay_games["sky_deck"] = None
        self.check_a24c("sky_deck", "no ")

    def test_a24c_dry_run_3_replays_consistent(self):
        o = Out(games=12, replays=3, dry=1)
        try:
            self.assertEqual([r[0] for r in o.replay_games["apex_circuit"]], ["0", "4", "8"])
            rc, lines, _, rows = o.run()
            self.assertEqual(rc, 0, lines[-1])
            self.assertTrue(lines[-1].startswith("GATE A2 (DRY RUN): no FAIL"), lines[-1])
            for t in TABLES:
                self.assertEqual(status_of(rows, t, "A2.4c"), ["PASS"])
            self.assertEqual(status_of(rows, "apex_circuit", "A2.4 sampled"), ["REPORT"])
        finally:
            o.close()

    def test_a24c_dry_run_mismatch_still_fails(self):
        o = Out(games=12, replays=3, dry=1)
        try:
            o.replay_games["feature_lab"][1][2] = "ffffffffffffffff"
            rc, lines, _, rows = o.run()
            self.assertEqual(rc, 1)
            self.assertEqual(status_of(rows, "feature_lab", "A2.4c"), ["FAIL"])
            self.assertTrue(lines[-1].startswith("GATE A2 (DRY RUN): 1 criteria failed"), lines[-1])
        finally:
            o.close()

    # A2.4a missing fields (finding 2)
    def test_a24a_missing_state_hash_in_both(self):
        for k in ("standard", "replay"):
            del self.out.reports[("apex_circuit", k)]["games"][5]["state_hash"]
        rows = self.check_fail("apex_circuit", "A2.4a (supplemental)")
        val = [r[3] for r in rows if r[0] == "FAIL"][0]
        self.assertIn("game 5: state_hash missing in full run, state_hash missing in re-run", val)

    def test_a24a_missing_seed_in_both(self):
        for k in ("standard", "replay"):
            self.out.reports[("feature_lab", k)]["games"][11]["seed"] = None
        rows = self.check_fail("feature_lab", "A2.4a (supplemental)")
        val = [r[3] for r in rows if r[0] == "FAIL"][0]
        self.assertIn("game 11: seed missing in full run, seed missing in re-run", val)

    def test_a24a_empty_state_hash_in_one(self):
        self.out.reports[("sky_deck", "replay")]["games"][0]["state_hash"] = ""
        rows = self.check_fail("sky_deck", "A2.4a (supplemental)")
        self.assertIn("game 0: state_hash missing in re-run", [r[3] for r in rows if r[0] == "FAIL"][0])

    # (b) each single criterion failing
    def test_stuck_ball(self):
        self.out.reports[("apex_circuit", "expert")]["findings"].append(
            {"kind": "StuckBall", "subject": "x", "game": 3, "tick": 9, "detail": "d"})
        self.check_fail("apex_circuit", "A2.1a zero stuck/endless/escaped/non-finite balls [expert]")

    def test_escaped_counter(self):
        self.out.reports[("sky_deck", "novice")]["counters"]["balls_escaped"] = 1
        self.check_fail("sky_deck", "A2.1a zero stuck/endless/escaped/non-finite balls [novice]")

    def test_script_error(self):
        r = self.out.reports[("feature_lab", "standard")]
        r["findings"].append({"kind": "ScriptError", "subject": "rules", "game": 1, "tick": 2, "detail": "d"})
        r["counters"]["script_faults"] = 1
        self.check_fail("feature_lab", "A2.1b zero script errors [standard]")

    def test_queue_overflow(self):
        r = self.out.reports[("feature_lab", "novice")]
        r["findings"].append({"kind": "QueueOverflow", "subject": "q", "game": 1, "tick": 2, "detail": "d"})
        r["counters"]["script_queue_overflows"] = 4
        self.check_fail("feature_lab", "A2.1c zero command-queue overflows [novice]")

    def test_switch_miss(self):
        r = self.out.reports[("apex_circuit", "novice")]
        r["coverage"]["switches"][1]["hits"] = 0
        r["findings"].append({"kind": "UnreachableSwitch", "subject": "sw_b", "game": None, "tick": 0, "detail": "d"})
        rows = self.check_fail("apex_circuit", "A2.2a 100 % switch coverage [novice]")
        self.assertTrue(any("sw_b" in r[3] for r in rows if r[0] == "FAIL"))

    def test_shot_miss(self):
        r = self.out.reports[("sky_deck", "expert")]
        r["coverage"]["shots"][2]["count"] = 0
        r["findings"].append({"kind": "ShotCoverage", "subject": "ramp", "game": None, "tick": 0, "detail": "d"})
        self.check_fail("sky_deck", "A2.2b every declared shot made [expert]")

    def test_mode_miss(self):
        r = self.out.reports[("feature_lab", "expert")]
        r["coverage"]["modes"][0]["starts"] = 0
        r["findings"].append({"kind": "ModeCoverage", "subject": "multiball", "game": None, "tick": 0, "detail": "d"})
        self.check_fail("feature_lab", "A2.2c every observable rule mode started [expert]")

    def test_unobservable_mode_is_reported_not_failed(self):
        r = self.out.reports[("feature_lab", "standard")]
        r["coverage"]["modes"].append({"id": "hidden", "starts": 0, "observable": False})
        r["findings"].append({"kind": "ModeCoverage", "subject": "hidden", "game": None, "tick": 0, "detail": "d"})
        rc, lines, vlines, rows = self.out.run()
        self.assertEqual(lines[-1], "GATE A2: PASS")
        rep = [x for x in rows if x[1] == "feature_lab" and "not observable" in x[2]]
        self.assertEqual([(x[0], x[3]) for x in rep], [("REPORT", "hidden")])

    def test_median_59s(self):
        for g in self.out.reports[("apex_circuit", "standard")]["games"]:
            g["ticks"] = 59_000
        self.check_fail("apex_circuit", "A2.3a")

    def test_median_361s(self):
        for g in self.out.reports[("apex_circuit", "standard")]["games"]:
            g["ticks"] = 361_000
        self.check_fail("apex_circuit", "A2.3a")

    def test_median_bounds_inclusive(self):
        for t, ticks in (("apex_circuit", 60_000), ("feature_lab", 360_000)):
            for g in self.out.reports[(t, "standard")]["games"]:
                g["ticks"] = ticks
        rc, lines, _, _ = self.out.run()
        self.assertEqual((rc, lines[-1]), (0, "GATE A2: PASS"))

    def test_median_is_nearest_rank(self):
        # 500 games at 59 s, 500 at 61 s: nearest rank ceil(0.5*1000)=500 -> 59 s
        # (an averaged median would be 60 s and pass).
        for g in self.out.reports[("sky_deck", "standard")]["games"]:
            g["ticks"] = 59_000 if g["game"] % 2 else 61_000
        rows = self.check_fail("sky_deck", "A2.3a")
        self.assertTrue(any(r[3].startswith("59.0 s") for r in rows if r[0] == "FAIL"))

    def test_capped_games_reported(self):
        for g in self.out.reports[("sky_deck", "standard")]["games"][:4]:
            g["end"] = "max_ticks"
        rc, lines, _, rows = self.out.run()
        self.assertEqual(rc, 0)
        r = [x for x in rows if x[1] == "sky_deck" and x[2] == "A2.3 game length [standard]"]
        self.assertEqual(r[0][0], "REPORT")
        self.assertIn("max_ticks=4", r[0][3])

    def test_novice_score_not_below_standard(self):
        self.out.reports[("feature_lab", "novice")]["distributions"]["game_score"]["median"] = 2000
        self.check_fail("feature_lab", "A2.3b")

    def test_standard_score_not_below_expert(self):
        self.out.reports[("apex_circuit", "standard")]["distributions"]["game_score"]["median"] = 3000
        self.check_fail("apex_circuit", "A2.3b")

    def test_games_999(self):
        r = self.out.reports[("sky_deck", "standard")]
        r["games"] = r["games"][:999]
        r["config"]["games"] = 999
        # Recordings consistent with the 999-game soak (spread over 999).
        self.out.replay_games["sky_deck"] = replay_rows(r, 20)
        self.check_fail("sky_deck", "A2.0 games >= 1000 [standard]")

    def test_soak_exit_status(self):
        write(os.path.join(self.out.dir, "apex_circuit", "soak_novice.rc"), "1\n")
        self.check_fail("apex_circuit", "A2.0 soak exit status [novice]")

    def test_build_failure(self):
        write(os.path.join(self.out.dir, "feature_lab", "build.rc"), "101\n")
        self.check_fail("feature_lab", "A2.0 bundle builds")

    def test_readiness_fail_and_missing(self):
        write(os.path.join(self.out.dir, "readiness.tsv"),
              "r6c\tFAIL\texit 101\nc6a\tPASS\tok\nc6c_recorded\tMISSING\tmissing test target spec_6_2_soak\n"
              "c6c_sampled\tPASS\tok\n")
        rc, lines, vlines, rows = self.out.run()
        self.assertEqual(rc, 1)
        fails = [r for r in rows if r[0] == "FAIL"]
        self.assertEqual(len(fails), 2)
        self.assertTrue(all(r[1] == "readiness" for r in fails))
        self.assertIn("missing test target", fails[1][3])

    # (c) verdict file's last line = printed last line, and --verdict-only is reproducible
    def test_verdict_file_matches_and_reproduces(self):
        self.out.reports[("feature_lab", "novice")]["distributions"]["game_score"]["median"] = 9999
        rc1, lines1, v1, _ = self.out.run()
        rc2, lines2, v2, _ = self.out.run()
        self.assertEqual(lines1[-1], v1[-1])
        self.assertEqual((rc1, lines1, v1), (rc2, lines2, v2))
        with open(os.path.join(self.out.dir, "gate_a2_table.txt"), encoding="utf-8") as f:
            self.assertEqual(f.read().rstrip("\n").split("\n")[-1], lines1[-1])

    # (d) incomplete
    def test_incomplete_two_tables(self):
        o = Out(tables=("apex_circuit", "feature_lab"))
        try:
            rc, lines, vlines, rows = o.run()
            self.assertEqual(rc, 1)
            self.assertTrue(lines[-1].startswith("GATE A2 INCOMPLETE: "), lines[-1])
            self.assertIn("sky_deck", lines[-1])
            self.assertEqual(lines[-1], vlines[-1])
            self.assertIn(("REPORT", "sky_deck", "table in TABLES", "SKIPPED (sky_deck not in TABLES)"), rows)
            self.assertTrue(all(r[0] == "REPORT" for r in rows if r[1] == "readiness"))
        finally:
            o.close()

    # dry run
    def test_dry_run(self):
        o = Out(games=20, replays=3, dry=1)
        try:
            rc, lines, vlines, rows = o.run()
            self.assertEqual(rc, 0, lines[-1])
            self.assertTrue(lines[-1].startswith("GATE A2 (DRY RUN): no FAIL"), lines[-1])
            self.assertEqual(status_of(rows, "apex_circuit", "A2.0 games"), ["REPORT"] * 3)
            self.assertEqual(status_of(rows, "apex_circuit", "A2.4 sampled"), ["REPORT"])
        finally:
            o.close()

    def test_dry_run_with_fail(self):
        o = Out(games=20, replays=3, dry=1)
        try:
            o.reports[("apex_circuit", "standard")]["coverage"]["shots"][0]["count"] = 0
            rc, lines, vlines, rows = o.run()
            self.assertEqual(rc, 1)
            self.assertTrue(lines[-1].startswith("GATE A2 (DRY RUN): 1 criteria failed"), lines[-1])
            self.assertNotIn("GATE A2: PASS", "\n".join(lines))
        finally:
            o.close()

    def test_full_run_games_20_never_passes(self):
        o = Out(games=20, replays=3, dry=0)
        try:
            rc, lines, _, _ = o.run()
            self.assertEqual(rc, 1)
            self.assertTrue(lines[-1].startswith("GATE A2: FAIL"), lines[-1])
        finally:
            o.close()


def load_verdict_module():
    """The verdict script as a module, without writing bytecode."""
    sys.dont_write_bytecode = True
    path = os.path.join(REPO, "scripts", "gate_a2_verdict.py")
    spec = importlib.util.spec_from_file_location("gate_a2_verdict_under_test", path)
    if spec is None or spec.loader is None:
        return None
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class DirFsyncTest(unittest.TestCase):
    """Finding 4: OUT is fsynced after the verdict files; a failure there is tolerated."""

    def test_verdict_tolerates_directory_fsync_failure(self):
        mod = load_verdict_module()
        if mod is None:
            self.skipTest("scripts/gate_a2_verdict.py is not importable")
        o = Out()
        real = os.fsync
        calls = {"dir": 0, "file": 0}

        def fake(fd):
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                calls["dir"] += 1
                raise OSError(22, "directory fsync not supported (test)")
            calls["file"] += 1
            return real(fd)

        devnull = open(os.devnull, "w", encoding="utf-8")
        saved = sys.stdout
        try:
            o.flush()
            os.fsync = fake
            sys.stdout = devnull
            rc = mod.main(["gate_a2_verdict.py", o.dir])
        finally:
            sys.stdout = saved
            os.fsync = real
            devnull.close()
        try:
            self.assertEqual(rc, 0)
            self.assertEqual(calls, {"dir": 1, "file": 2})
            for name in ("gate_a2_verdict.txt", "gate_a2_table.txt"):
                with open(os.path.join(o.dir, name), encoding="utf-8") as f:
                    self.assertEqual(f.read().rstrip("\n").split("\n")[-1], "GATE A2: PASS")
        finally:
            o.close()

    def test_verdict_fsyncs_the_directory(self):
        mod = load_verdict_module()
        if mod is None:
            self.skipTest("scripts/gate_a2_verdict.py is not importable")
        o = Out(tables=("apex_circuit",), games=12, replays=3, dry=1)
        real = os.fsync
        dirs = []

        def spy(fd):
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                dirs.append(os.fstat(fd).st_ino)
            return real(fd)

        devnull = open(os.devnull, "w", encoding="utf-8")
        saved = sys.stdout
        try:
            o.flush()
            os.fsync = spy
            sys.stdout = devnull
            rc = mod.main(["gate_a2_verdict.py", o.dir])
        finally:
            sys.stdout = saved
            os.fsync = real
            devnull.close()
        try:
            self.assertEqual(rc, 0)
            self.assertEqual(dirs, [os.stat(o.dir).st_ino])
        finally:
            o.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
