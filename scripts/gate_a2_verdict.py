#!/usr/bin/env python3
"""Gate A2 verdict (PLAN Gate A2, table soak), computed from artifacts only.

Run it as ``python3 -I scripts/gate_a2_verdict.py OUT``. It reads what
``scripts/gate_a2.sh`` left under OUT and never runs a simulation, so the
verdict can be rebuilt from the artifacts at any time
(``scripts/gate_a2.sh --verdict-only OUT``) and tested on hand-made JSON.

Inputs under OUT (all written by gate_a2.sh):

* ``gate_a2_run.txt``: ``key=value`` lines: dry, games, replays, seed,
  threads, tables (space separated), readiness, record_games (1 when the
  pin_sim binary has ``soak --record-games``; 0 when it does not).
* ``<t>/build.rc``: exit status of ``tables/<t>/build.sh``.
* ``<t>/soak_<skill>.rc`` and ``<t>/soak_<skill>/soak_report.json`` for
  skill in novice, standard, expert (``pin_sim soak``).
* ``<t>/replay_games.tsv``: ``game<TAB>soak_state_hash<TAB>replay_state_hash
  <TAB>soak_score<TAB>replay_score<TAB>ticks`` per sampled standard game
  (``pin_sim soak --record-games spread:REPLAYS``, then ``pin_sim <bundle>
  --replay-game FILE``, which replays the recorded bot input without the bot).
  This is A2.4c, the gating sampled-game replay.
* ``<t>/replay_soak.rc`` and ``<t>/replay_soak/soak_report.json``: the
  standard soak re-run for the first REPLAYS games with another thread count
  (A2.4a, supplemental).
* ``<t>/replay_pattern.tsv``: ``seed<TAB>recorded<TAB>replayed`` per seed
  (``pin_sim --record-inputs`` / ``--replay-inputs`` state hashes).
* ``readiness.tsv``: ``id<TAB>status<TAB>detail`` per readiness test.

Outputs: ``OUT/gate_a2_verdict.txt`` (one ``STATUS<TAB>table<TAB>criterion
<TAB>value`` line per criterion, then the final line) and
``OUT/gate_a2_table.txt`` (the human table, then the final line). Both are
written and flushed before the final line is printed on stdout. Exit 0 for
``GATE A2: PASS`` and for a dry run without FAIL, 1 otherwise.

Field names follow ``SoakReport::to_json`` (crates/pin_sim/src/autoplay/
report.rs, schema 1); the median is the nearest-rank quantile of
``nearest_rank`` there (rank ceil(q*n), at least 1).
"""

import json
import math
import os
import sys

SKILLS = ("novice", "standard", "expert")
# PLAN Gate A2 "the three tables".
REQUIRED_TABLES = ("apex_circuit", "feature_lab", "sky_deck")
GATE_GAMES = 1000
GATE_REPLAYS = 20
# Median game length at `standard`, ticks (1 tick = 1 ms, pin_core TICK_NS).
MEDIAN_MIN_TICKS = 60_000
MEDIAN_MAX_TICKS = 360_000
TICKS_PER_S = 1000.0
# FindingKind::fails_soak, grouped by criterion.
BALL_KINDS = ("StuckBall", "EndlessBall", "EscapedBall", "NonFinite")
# FindingKind::LongBall: a ball past the 300 s endless limit that later
# drained naturally after broad play. Reported, never failing
# (docs/decisions/2026-11-19-fix-a2-longball.md).
LONG_BALL_KINDS = ("LongBall",)
SCRIPT_KINDS = ("ScriptError",)
QUEUE_KINDS = ("QueueOverflow",)
THIN_SHOTS = 3
HEX16 = frozenset("0123456789abcdef")
# Context REPORT lines the brief names (docs/queue/6.1.md).
APEX_NOT_SOAK_COVERED = (
    "not soak-covered (covered by apex_game.rs / spec_6_1_rules.rs): "
    "Firewall Save, Apex Jackpot [6.1-a2-modes]"
)


def nearest_rank(sorted_values, q):
    """report.rs nearest_rank: the value at rank ceil(q*n), 1-based, >= 1."""
    n = len(sorted_values)
    rank = math.ceil(q * n)
    rank = min(max(rank, 1), n)
    return sorted_values[rank - 1]


def secs(ticks):
    return "%.1f s" % (ticks / TICKS_PER_S)


def read_text(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError:
        return None


def read_rc(path):
    text = read_text(path)
    if text is None:
        return None
    try:
        return int(text.strip())
    except ValueError:
        return None


def read_json(path):
    text = read_text(path)
    if text is None:
        return None, "missing %s" % path
    try:
        return json.loads(text), None
    except ValueError as e:
        return None, "unreadable %s (%s)" % (path, e)


def read_run(out):
    run = {}
    text = read_text(os.path.join(out, "gate_a2_run.txt"))
    if text is not None:
        for line in text.splitlines():
            if "=" in line:
                k, v = line.split("=", 1)
                run[k.strip()] = v.strip()
    return run


def as_int(v, default):
    try:
        return int(v)
    except (TypeError, ValueError):
        return default


class Verdict:
    def __init__(self, dry):
        self.dry = dry
        self.rows = []

    def add(self, status, table, crit, value):
        self.rows.append((status, table, crit, str(value)))

    def fails(self):
        return [r for r in self.rows if r[0] == "FAIL"]


def kind_count(report, kinds):
    n = {k: 0 for k in kinds}
    for f in report.get("findings", []):
        if f.get("kind") in n:
            n[f["kind"]] += 1
    return n


def subjects(report, kind):
    return sorted({f.get("subject", "?") for f in report.get("findings", []) if f.get("kind") == kind})


def fmt_counts(d):
    return ", ".join("%s=%d" % (k, v) for k, v in d.items())


def judge_soundness(v, t, skill, r):
    c = r.get("counters", {})
    ball = kind_count(r, BALL_KINDS)
    esc = as_int(c.get("balls_escaped"), -1)
    ok = all(n == 0 for n in ball.values()) and esc == 0
    v.add("PASS" if ok else "FAIL", t, "A2.1a zero stuck/endless/escaped/non-finite balls [%s]" % skill,
          "%s, counters.balls_escaped=%d" % (fmt_counts(ball), esc))
    lb = kind_count(r, LONG_BALL_KINDS)
    lb_subjects = subjects(r, "LongBall")
    v.add("REPORT", t, "A2.1a long balls (>300 s, drained, broad play; reported, not failing) [%s]" % skill,
          fmt_counts(lb) + ((" (%s)" % ", ".join(lb_subjects)) if lb_subjects else ""))
    se = kind_count(r, SCRIPT_KINDS)
    faults = as_int(c.get("script_faults"), -1)
    ok = se["ScriptError"] == 0 and faults == 0
    v.add("PASS" if ok else "FAIL", t, "A2.1b zero script errors [%s]" % skill,
          "ScriptError=%d, counters.script_faults=%d" % (se["ScriptError"], faults))
    qo = kind_count(r, QUEUE_KINDS)
    ovf = as_int(c.get("script_queue_overflows"), -1)
    ok = qo["QueueOverflow"] == 0 and ovf == 0
    v.add("PASS" if ok else "FAIL", t, "A2.1c zero command-queue overflows [%s]" % skill,
          "QueueOverflow=%d, counters.script_queue_overflows=%d" % (qo["QueueOverflow"], ovf))


def judge_coverage(v, t, skill, r):
    cov = r.get("coverage", {})
    sw = cov.get("switches", [])
    missing = sorted({s["name"] for s in sw if as_int(s.get("hits"), 0) <= 0}
                     | set(subjects(r, "UnreachableSwitch")))
    v.add("PASS" if sw and not missing else "FAIL", t, "A2.2a 100 %% switch coverage [%s]" % skill,
          "%d/%d switches hit%s" % (len(sw) - len([s for s in sw if s["name"] in missing]), len(sw),
                                    "; never hit: " + ", ".join(missing) if missing else
                                    ("" if sw else "; no switches in report")))
    shots = cov.get("shots", [])
    missing = sorted({s["id"] for s in shots if as_int(s.get("count"), 0) <= 0}
                     | set(subjects(r, "ShotCoverage")))
    v.add("PASS" if not missing else "FAIL", t, "A2.2b every declared shot made [%s]" % skill,
          "%d/%d shots made%s" % (len([s for s in shots if s["id"] not in missing]), len(shots),
                                  "; never made: " + ", ".join(missing) if missing else ""))
    modes = cov.get("modes", [])
    observable = {m["id"]: m for m in modes if m.get("observable") is True}
    unobservable = sorted(m["id"] for m in modes if m.get("observable") is not True)
    mode_findings = set(subjects(r, "ModeCoverage"))
    missing = sorted({i for i, m in observable.items() if as_int(m.get("starts"), 0) <= 0}
                     | (mode_findings - set(unobservable)))
    v.add("PASS" if not missing else "FAIL", t, "A2.2c every observable rule mode started [%s]" % skill,
          "%d/%d observable modes started%s" % (len(observable) - len([i for i in observable if i in missing]),
                                                len(observable),
                                                "; never started: " + ", ".join(missing) if missing else ""))
    if unobservable:
        v.add("REPORT", t, "A2.2c modes not observable (no persist key mode:<id>), not soak-judged [%s]" % skill,
              ", ".join(unobservable))


def is_hash(s):
    return len(s) == 16 and set(s) <= HEX16


def is_score(s):
    if s == "none":
        return True
    t = s[1:] if s.startswith("-") else s
    return t.isdigit() and t.isascii()


def report_score(g):
    """A game_list score as the tsv/stdout writes it: decimal, or none."""
    sc = g.get("score")
    if sc is None:
        return "none"
    if isinstance(sc, bool) or not isinstance(sc, int):
        return None
    return str(sc)


def spread(games, n):
    """pin_sim --record-games spread:N: floor(i*GAMES/N) for i = 0..N-1."""
    return [i * games // n for i in range(n)]


def judge_sampled_replay(v, t, tdir, st, run, replays):
    """A2.4c: sampled standard games replayed from recorded input, no bot."""
    path = os.path.join(tdir, "replay_games.tsv")
    text = read_text(path)
    rows, bad = [], []
    for n, line in enumerate((text or "").splitlines(), 1):
        parts = line.split("\t")
        if len(parts) != 6 or not parts[0].isdigit() or not parts[0].isascii():
            bad.append("line %d unparseable" % n)
            continue
        rows.append(parts)
    if text is None and v.dry and run.get("record_games") == "0":
        v.add("REPORT", t, "A2.4c sampled bot games replayed from recorded input without the bot",
              "REPORT (dry run): the pin_sim binary has no soak --record-games, no replay data")
        return
    ok_n = 0
    if text is None:
        bad.insert(0, "no %s" % path)
    elif st is None:
        bad.insert(0, "no standard soak report")
    if st is not None and text is not None:
        games = as_int(st.get("config", {}).get("games"), len(st.get("games", [])))
        want = spread(games, replays) if 1 <= replays <= games else None
        got = [int(p[0]) for p in rows]
        if want is None:
            bad.insert(0, "REPLAYS=%d not in 1..GAMES=%d" % (replays, games))
        elif got != want:
            extra = [g for g in got if g not in want]
            missing = [g for g in want if g not in got]
            bad.insert(0, "%d rows, games %s; expected %d (spread floor(i*%d/%d))%s%s" % (
                len(got), "none" if not got else ",".join(str(g) for g in got[:5]) + (",..." if len(got) > 5 else ""),
                replays, games, replays,
                "; missing " + ",".join(str(g) for g in missing[:3]) if missing else "",
                "; unexpected " + ",".join(str(g) for g in extra[:3]) if extra else ""))
        listed = {}
        for g in st.get("games", []):
            listed.setdefault(as_int(g.get("game"), -1), g)
        for p in rows:
            gi, sh, rh, sc, rs = int(p[0]), p[1], p[2], p[3], p[4]
            g = listed.get(gi)
            why = []
            if g is None:
                why.append("not in the standard soak report")
            else:
                rep_sh, rep_sc = g.get("state_hash"), report_score(g)
                if not isinstance(rep_sh, str) or not is_hash(rep_sh):
                    why.append("report state_hash %r unusable" % (rep_sh,))
                elif sh != rep_sh:
                    why.append("recorded soak state_hash %r != report %s" % (sh, rep_sh))
                if rep_sc is None:
                    why.append("report score %r unusable" % (g.get("score"),))
                elif sc != rep_sc:
                    why.append("recorded soak score %r != report %s" % (sc, rep_sc))
            if not is_hash(rh):
                why.append("replay state_hash %r" % rh if rh else "no replay state_hash")
            elif rh != sh:
                why.append("replay state_hash %s != soak %s" % (rh, sh or "''"))
            if not is_score(rs):
                why.append("replay score %r" % rs if rs else "no replay score")
            elif rs != sc:
                why.append("replay score %s != soak %s" % (rs, sc or "''"))
            if why:
                bad.append("game %d: %s" % (gi, ", ".join(why)))
            else:
                ok_n += 1
    if replays < GATE_REPLAYS and not v.dry:
        bad.insert(0, "REPLAYS=%d < %d" % (replays, GATE_REPLAYS))
    v.add("FAIL" if bad else "PASS", t,
          "A2.4c sampled bot games replayed from recorded input without the bot: "
          "N of REPLAYS identical state_hash and score",
          "%d of %d identical%s" % (ok_n, replays, "; " + "; ".join(bad[:3]) if bad else ""))


def game_ticks(r):
    return [as_int(g.get("ticks"), 0) for g in r.get("games", [])]


def judge_table(v, out, t, run):
    replays = as_int(run.get("replays"), GATE_REPLAYS)
    tdir = os.path.join(out, t)
    rc = read_rc(os.path.join(tdir, "build.rc"))
    if rc == 0:
        v.add("PASS", t, "A2.0 bundle builds and validates", "build.sh exit 0")
    else:
        v.add("FAIL", t, "A2.0 bundle builds and validates",
              "no build record" if rc is None else "build.sh exit %d (see %s/logs/build.log)" % (rc, tdir))

    reports = {}
    for skill in SKILLS:
        r, err = read_json(os.path.join(tdir, "soak_%s" % skill, "soak_report.json"))
        src = "soak_%s" % skill
        if r is None:
            for crit in ("A2.0 games >= %d" % GATE_GAMES, "A2.0 soak exit status",
                         "A2.1 soundness", "A2.2 coverage"):
                v.add("FAIL", t, "%s [%s]" % (crit, skill), "no soak report: %s" % err)
            continue
        reports[skill] = r
        n = len(r.get("games", []))
        cfg_games = as_int(r.get("config", {}).get("games"), -1)
        value = "%d games (config.games=%d)" % (n, cfg_games)
        if n >= GATE_GAMES and cfg_games >= GATE_GAMES:
            v.add("PASS", t, "A2.0 games >= %d [%s]" % (GATE_GAMES, skill), value)
        elif v.dry:
            v.add("REPORT", t, "A2.0 games >= %d [%s]" % (GATE_GAMES, skill), value + " (dry run)")
        else:
            v.add("FAIL", t, "A2.0 games >= %d [%s]" % (GATE_GAMES, skill), value)
        src_rc = read_rc(os.path.join(tdir, "soak_%s.rc" % skill))
        if src_rc == 0:
            v.add("PASS", t, "A2.0 soak exit status [%s]" % skill, "pin_sim soak exit 0")
        else:
            v.add("FAIL", t, "A2.0 soak exit status [%s]" % skill,
                  "no exit status recorded" if src_rc is None else
                  "pin_sim soak exit %d (see %s/logs/%s.log)" % (src_rc, tdir, src))
        judge_soundness(v, t, skill, r)
        judge_coverage(v, t, skill, r)
        wall = r.get("timing", {}).get("wall_s")
        if wall is not None:
            v.add("REPORT", t, "soak wall time [%s]" % skill, "%s s, %s threads" % (wall, r.get("timing", {}).get("threads")))

    # Context lines from docs/queue/6.1.md (the brief's only table-specific lines).
    if t == "apex_circuit":
        v.add("REPORT", t, "A2.2 modes", APEX_NOT_SOAK_COVERED)
        for skill in SKILLS:
            r = reports.get(skill)
            if r is None:
                continue
            for s in r.get("coverage", {}).get("shots", []):
                if s.get("id") == "drop_bank":
                    v.add("REPORT", t, "A2.2b drop_bank count [%s]" % skill,
                          "%s (may be over-credited across a drain [6.1-drop-bank-soak]; not gating)" % s.get("count"))
    ex = reports.get("expert")
    if ex is not None:
        shots = sorted(ex.get("coverage", {}).get("shots", []),
                       key=lambda s: (as_int(s.get("count"), 0), s.get("id", "")))
        thin = ", ".join("%s=%s" % (s.get("id"), s.get("count")) for s in shots[:THIN_SHOTS])
        v.add("REPORT", t, "A2.2b thinnest shot margins at expert [6.1-expert-orbits]", thin or "no shots declared")

    # A2.3a median game length at standard.
    st = reports.get("standard")
    if st is None:
        v.add("FAIL", t, "A2.3a median game length at standard in 60..360 s", "no standard report")
    else:
        ticks = sorted(game_ticks(st))
        if not ticks:
            v.add("FAIL", t, "A2.3a median game length at standard in 60..360 s", "no games in report")
        else:
            med = nearest_rank(ticks, 0.5)
            ok = MEDIAN_MIN_TICKS <= med <= MEDIAN_MAX_TICKS
            v.add("PASS" if ok else "FAIL", t, "A2.3a median game length at standard in 60..360 s",
                  "%s (%d ticks, %d games)" % (secs(med), med, len(ticks)))
    for skill in SKILLS:
        r = reports.get(skill)
        if r is None:
            continue
        ends = {}
        for g in r.get("games", []):
            ends[g.get("end", "?")] = ends.get(g.get("end", "?"), 0) + 1
        other = {k: n for k, n in sorted(ends.items()) if k != "game_over"}
        ticks = sorted(game_ticks(r))
        med = secs(nearest_rank(ticks, 0.5)) if ticks else "n/a"
        v.add("REPORT", t, "A2.3 game length [%s]" % skill,
              "median %s; games not ended by game_over (counted at their capped length): %s"
              % (med, ", ".join("%s=%d" % kv for kv in other.items()) if other else "0"))

    # A2.3b scores increase with skill.
    meds = []
    for skill in SKILLS:
        r = reports.get(skill)
        d = (r or {}).get("distributions", {}).get("game_score")
        meds.append(None if not d or as_int(d.get("count"), 0) == 0 else as_int(d.get("median"), None))
    ok = None not in meds and meds[0] < meds[1] < meds[2]
    shown = "medians " + ", ".join("%s %s" % (s, "n/a" if m is None else m) for s, m in zip(SKILLS, meds))
    shown += "; strictly increasing: %s" % ("yes" if ok else "no")
    v.add("PASS" if ok else "FAIL", t, "A2.3b median score novice < standard < expert", shown)

    # A2.4 deterministic replay.
    if replays < GATE_REPLAYS:
        v.add("REPORT" if v.dry else "FAIL", t, "A2.4 sampled replays >= %d" % GATE_REPLAYS,
              "REPLAYS=%d%s" % (replays, " (dry run)" if v.dry else ""))
    else:
        v.add("PASS", t, "A2.4 sampled replays >= %d" % GATE_REPLAYS, "REPLAYS=%d" % replays)
    judge_sampled_replay(v, t, tdir, st, run, replays)
    crit = ("A2.4a (supplemental) soak games 0..%d re-run alone (other thread count) equal the full standard run "
            "(seed + state_hash per game)" % (replays - 1))
    rep, err = read_json(os.path.join(tdir, "replay_soak", "soak_report.json"))
    if st is None or rep is None:
        v.add("FAIL", t, crit, "no full standard report" if st is None else "no re-run report: %s" % err)
    else:
        full = {as_int(g.get("game"), -1): g for g in st.get("games", [])}
        again = {as_int(g.get("game"), -1): g for g in rep.get("games", [])}
        bad = []
        for i in range(replays):
            a, b = full.get(i), again.get(i)
            if a is None or b is None:
                bad.append("game %d missing (%s)" % (i, "full run" if a is None else "re-run"))
            else:
                # A field missing, None or empty in either report is a
                # mismatch, never "equal" (None == None).
                absent = ["%s missing in %s" % (k, w) for k in ("seed", "state_hash")
                          for w, g in (("full run", a), ("re-run", b)) if g.get(k) in (None, "")]
                if absent:
                    bad.append("game %d: %s" % (i, ", ".join(absent)))
                elif (a["seed"], a["state_hash"]) != (b["seed"], b["state_hash"]):
                    bad.append("game %d: %s/%s vs %s/%s" % (i, a["seed"], a["state_hash"],
                                                             b["seed"], b["state_hash"]))
        threads = "threads %s vs %s" % (st.get("timing", {}).get("threads"), rep.get("timing", {}).get("threads"))
        v.add("PASS" if not bad and replays > 0 else "FAIL", t, crit,
              ("%d/%d equal, %s" % (replays - len(bad), replays, threads)) + ("; " + "; ".join(bad[:3]) if bad else ""))
    crit = "A2.4b (supplemental) built-in input pattern recorded and replayed: equal state_hash per seed"
    text = read_text(os.path.join(tdir, "replay_pattern.tsv"))
    rows = []
    for line in (text or "").splitlines():
        parts = line.split("\t")
        if len(parts) == 3:
            rows.append(parts)
    if len(rows) < max(replays, 1):
        v.add("FAIL", t, crit, "%d of %d seeds recorded" % (len(rows), replays))
    else:
        bad = [p for p in rows if not p[1] or p[1] != p[2]]
        v.add("PASS" if not bad else "FAIL", t, crit,
              "%d/%d seeds equal%s" % (len(rows) - len(bad), len(rows),
                                       "; " + "; ".join("seed %s: %s vs %s" % tuple(p) for p in bad[:3]) if bad else ""))


READINESS = (
    ("R6c sky_deck standard median ball time >= 25 s (25..100 s), novice/expert >= 20 s "
     "[sky_deck_soak::r6c_ball_times_200_games_per_skill]", "r6c"),
    ("C6a 120-game readiness soak incl. score order by skill "
     "[spec_6_2_soak::c6a_soak_readiness_40_games_per_skill]", "c6a"),
    ("C6c recorded inputs replay identically 10 times (seed-repeatability check today) "
     "[spec_6_2_soak::c6c_recorded_inputs_replay_identically_10_times]", "c6c_recorded"),
    ("C6c sampled bot games replay identically (seed-repeatability check today) "
     "[spec_6_2_soak::c6c_sampled_bot_games_replay_identically]", "c6c_sampled"),
)


def judge_readiness(v, out, tables):
    text = read_text(os.path.join(out, "readiness.tsv")) or ""
    got = {}
    for line in text.splitlines():
        parts = line.split("\t", 2)
        if len(parts) >= 2:
            got[parts[0]] = (parts[1], parts[2] if len(parts) > 2 else "")
    for name, key in READINESS:
        if "sky_deck" not in tables:
            v.add("REPORT", "readiness", name, "SKIPPED (sky_deck not in TABLES)")
            continue
        status, detail = got.get(key, ("NONE", "no result recorded"))
        if status == "PASS":
            v.add("PASS", "readiness", name, detail)
        elif status == "MISSING":
            v.add("REPORT" if v.dry else "FAIL", "readiness", name,
                  ("REPORT (missing, dry run): " if v.dry else "FAIL (missing test target): ") + detail)
        elif status == "SKIPPED" and v.dry:
            v.add("SKIPPED", "readiness", name, detail)
        else:
            v.add("FAIL", "readiness", name, "%s %s" % (status, detail))


def human_table(v):
    lines = ["================ Gate A2 ================"]
    current = None
    for status, table, crit, value in v.rows:
        if table != current:
            lines.append("-- %s" % table)
            current = table
        lines.append("%-7s %s  %s" % (status, crit, value))
    lines.append("=========================================")
    return lines


def fsync_dir(path):
    """fsync a directory so the new verdict files' entries are durable.

    Platforms that cannot open or fsync a directory (OSError, or no
    os.O_RDONLY/os.fsync: AttributeError) are tolerated: the files themselves
    were already fsynced, and the verdict must not fail for this.
    """
    try:
        fd = os.open(path, os.O_RDONLY)
    except (OSError, AttributeError):
        return
    try:
        os.fsync(fd)
    except (OSError, AttributeError):
        pass
    finally:
        os.close(fd)


def main(argv):
    if len(argv) != 2:
        sys.stderr.write("usage: python3 -I gate_a2_verdict.py OUT\n")
        return 2
    out = argv[1]
    run = read_run(out)
    if run:
        dry = run.get("dry", "0") == "1"
        tables = sorted(run.get("tables", "").split())
    else:
        # No run record: judge what is there as a full run.
        dry = False
        tables = sorted(d for d in os.listdir(out)
                        if os.path.isdir(os.path.join(out, d)) and
                        any(os.path.isdir(os.path.join(out, d, "soak_%s" % s)) for s in SKILLS))
    v = Verdict(dry)
    games = as_int(run.get("games"), GATE_GAMES)
    replays = as_int(run.get("replays"), GATE_REPLAYS)
    v.add("REPORT", "run", "parameters",
          "GAMES=%d REPLAYS=%d SEED=%s THREADS=%s DRY=%d TABLES=%s%s" % (
              games, replays, run.get("seed", "?"), run.get("threads", "?"), dry, " ".join(tables) or "(none)",
              "" if run else " (no gate_a2_run.txt: judged as a full run)"))
    missing = [t for t in REQUIRED_TABLES if t not in tables]
    for t in missing:
        v.add("REPORT", t, "table in TABLES", "SKIPPED (%s not in TABLES)" % t)
    for t in tables:
        try:
            judge_table(v, out, t, run)
        except (KeyError, TypeError, ValueError, AttributeError) as e:
            # A malformed report must not lose the verdict: it is a FAIL.
            v.add("FAIL", t, "A2 report readable", "verdict error on malformed artifacts: %r" % (e,))
    judge_readiness(v, out, tables)

    fails = v.fails()
    first = ", ".join("%s %s" % (r[1], r[2]) for r in fails[:3])
    if dry:
        final = "GATE A2 (DRY RUN): %s (%d FAIL, %d PASS, %d REPORT/SKIPPED; GAMES=%d REPLAYS=%d; not a gate verdict)" % (
            "no FAIL" if not fails else "%d criteria failed: %s" % (len(fails), first),
            len(fails), len([r for r in v.rows if r[0] == "PASS"]),
            len([r for r in v.rows if r[0] in ("REPORT", "SKIPPED")]), games, replays)
        rc = 0 if not fails else 1
    elif missing:
        final = "GATE A2 INCOMPLETE: tables not run: %s%s" % (
            ", ".join(missing), "; %d criteria failed: %s" % (len(fails), first) if fails else "")
        rc = 1
    elif fails:
        final = "GATE A2: FAIL (%d criteria failed: %s)" % (len(fails), first)
        rc = 1
    else:
        final = "GATE A2: PASS"
        rc = 0

    table = human_table(v)
    with open(os.path.join(out, "gate_a2_verdict.txt"), "w", encoding="utf-8") as f:
        for row in v.rows:
            f.write("\t".join(x.replace("\t", " ").replace("\n", " ") for x in row) + "\n")
        f.write(final + "\n")
        f.flush()
        os.fsync(f.fileno())
    with open(os.path.join(out, "gate_a2_table.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(table) + "\n" + final + "\n")
        f.flush()
        os.fsync(f.fileno())
    fsync_dir(out)
    sys.stdout.write("\n".join(table) + "\n")
    sys.stdout.flush()
    sys.stdout.write(final + "\n")
    sys.stdout.flush()
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
