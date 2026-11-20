"""Text-level checks of .github/workflows/release.yml (stdlib only, comments stripped)."""
import re
import unittest

from support import RELEASE_YML, strip_yaml_comments

ALLOWED_USES = {
    "actions/checkout@v5", "Swatinem/rust-cache@v2", "dtolnay/rust-toolchain@1.97.0",
    "actions/setup-dotnet@v4", "actions/upload-artifact@v4", "actions/download-artifact@v4",
}


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def block(lines, header_re, indent):
    """Lines under the first line matching header_re at the given indent (exclusive of header)."""
    out, inside = [], False
    for ln in lines:
        if not inside:
            if indent_of(ln) == indent and re.match(header_re, ln.strip()):
                inside = True
            continue
        if ln.strip() and indent_of(ln) <= indent:
            break
        out.append(ln)
    return out if inside else None


def split_jobs(lines):
    body = block(lines, r"jobs:\s*$", 0)
    jobs, cur = {}, None
    for ln in body or []:
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", ln)
        if m:
            cur = m.group(1)
            jobs[cur] = []
        elif cur is not None:
            jobs[cur].append(ln)
    return {k: "\n".join(v) for k, v in jobs.items()}


class ReleaseWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        assert RELEASE_YML.is_file(), ".github/workflows/release.yml missing"
        cls.raw = RELEASE_YML.read_text(encoding="utf-8")
        cls.text = strip_yaml_comments(cls.raw)
        cls.lines = cls.text.split("\n")
        cls.jobs = split_jobs(cls.lines)
        cls.logical = re.sub(r"\\\s*\n\s*", " ", cls.text)

    def job_if(self, job):
        m = re.search(r"^\s{4}if:\s*(.+)$", self.jobs[job], re.M)
        return m.group(1) if m else None

    def is_non_pr(self, name, seen=()):
        cond = self.job_if(name)
        if cond and re.search(r"!=\s*['\"]pull_request['\"]|ref_type\s*==\s*['\"]tag['\"]|refs/tags/"
                              r"|event_name\s*==\s*['\"](push|workflow_dispatch)['\"]", cond):
            return True
        m = re.search(r"^\s{4}needs:\s*(.+)$", self.jobs[name], re.M)
        needs = []
        if m:
            needs = re.findall(r"[A-Za-z0-9_-]+", m.group(1))
        else:
            nb = block(self.jobs[name].split("\n"), r"needs:\s*$", 4)
            for ln in nb or []:
                needs += re.findall(r"[A-Za-z0-9_-]+", ln)
        return any(n in self.jobs and n not in seen and self.is_non_pr(n, seen + (name,)) for n in needs)

    def test_name_is_release(self):
        self.assertRegex(self.text, r"(?m)^name:\s*['\"]?release['\"]?\s*$")

    def test_triggers(self):
        on = block(self.lines, r"""['"]?on['"]?:\s*$""", 0)
        self.assertIsNotNone(on, "no top-level on: block")
        on_text = "\n".join(on)
        pr = block(on, r"pull_request:\s*$", 2)
        self.assertIsNotNone(pr, "pull_request trigger missing")
        pr_text = "\n".join(pr)
        self.assertIn("paths", pr_text)
        for p in ("scripts/package.py", "tables/**", "tools/dof_bridge/**", ".github/workflows/release.yml"):
            self.assertIn(p, pr_text, p)
        self.assertRegex(on_text, r"(?m)^\s{2}workflow_dispatch:")
        push = block(on, r"push:\s*$", 2)
        self.assertIsNotNone(push, "push trigger missing")
        push_text = "\n".join(push)
        self.assertRegex(push_text, r"tags")
        self.assertRegex(push_text, r"""['"]?v\*['"]?""")
        self.assertNotIn("branches", push_text)

    def test_permissions_read_by_default_write_exactly_once(self):
        perms = block(self.lines, r"permissions:\s*$", 0)
        self.assertIsNotNone(perms, "top-level permissions: block missing")
        self.assertRegex("\n".join(perms), r"contents:\s*read")
        self.assertNotIn("write", "\n".join(perms))
        self.assertEqual(len(re.findall(r"contents:\s*write", self.text)), 1)

    def test_contents_write_only_in_the_draft_release_job(self):
        holders = [j for j, t in self.jobs.items() if re.search(r"contents:\s*write", t)]
        self.assertEqual(len(holders), 1, holders)
        self.assertIn("gh release create", self.jobs[holders[0]])

    def test_forbidden_strings_absent(self):
        for s in ("secrets.", "notarytool", "signtool", "cargo publish", "docker push", "git push",
                  "git tag", "--latest", "gh release edit"):
            self.assertNotIn(s, self.text, s)

    def test_every_gh_release_create_is_draft(self):
        cmds = [ln for ln in self.logical.split("\n") if "gh release create" in ln]
        self.assertTrue(cmds, "no gh release create found")
        for c in cmds:
            self.assertIn("--draft", c, c)

    def test_pull_request_job_runs_package_dry_run_on_linux_target(self):
        found = []
        for name, t in self.jobs.items():
            if "scripts/package.py" in t and "--dry-run" in t and "x86_64-unknown-linux-gnu" in t:
                found.append(name)
        self.assertTrue(found, "no job runs package.py --dry-run for linux")
        self.assertTrue(any(not self.is_non_pr(j) or self.job_if(j) is None for j in found),
                        "dry-run job must run on pull requests")
        for j in found:
            cond = self.job_if(j) or ""
            self.assertNotRegex(cond, r"!=\s*['\"]pull_request['\"]")
            self.assertNotRegex(cond, r"ref_type\s*==\s*['\"]tag['\"]|refs/tags/")

    def test_cargo_build_and_test_jobs_exist_not_pr_triggered_and_deny_warnings(self):
        for cmd in ("cargo build --release --workspace", "cargo test --release --workspace"):
            holders = [j for j, t in self.jobs.items() if cmd in t]
            self.assertTrue(holders, cmd)
            for j in holders:
                self.assertTrue(self.is_non_pr(j), "%s (%s) may run on pull requests" % (j, cmd))
                scope = self.jobs[j] + "\n" + "\n".join(block(self.lines, r"env:\s*$", 0) or [])
                self.assertRegex(scope, r"RUSTFLAGS:.*-D\s+warnings", (j, cmd))

    def test_matrix_covers_three_operating_systems(self):
        for os_name in ("ubuntu-latest", "windows-latest", "macos-latest"):
            self.assertIn(os_name, self.text, os_name)

    def test_macos_toolchain_step_lists_x86_64_apple_darwin(self):
        steps = list(re.finditer(r"(?m)^(\s*)- (?:name:.*\n\s*)?uses:\s*dtolnay/rust-toolchain@1\.97\.0.*$", self.text))
        self.assertTrue(steps, "no rust-toolchain steps")
        good = False
        for m in steps:
            ind = len(m.group(1))
            rest = self.text[m.end():]
            nxt = re.search(r"(?m)^\s{0,%d}- " % ind, rest)
            step_text = rest[:nxt.start()] if nxt else rest
            if "x86_64-apple-darwin" in step_text:
                good = True
        self.assertTrue(good, "no rust-toolchain step lists x86_64-apple-darwin")
        # the macOS job (or matrix) must be tied to that target
        self.assertRegex(self.text, r"macos-latest[\s\S]*x86_64-apple-darwin|x86_64-apple-darwin[\s\S]*macos-latest")

    def test_uses_only_pinned_refs_already_in_repo_workflows(self):
        uses = re.findall(r"(?m)^\s*-?\s*uses:\s*(\S+)", self.text)
        self.assertTrue(uses)
        for u in uses:
            self.assertIn(u, ALLOWED_USES, u)

    def test_draft_release_job_gated_on_tag_push_not_dispatch(self):
        holders = [j for j, t in self.jobs.items() if "gh release create" in t]
        self.assertTrue(holders)
        for j in holders:
            cond = self.job_if(j)
            self.assertIsNotNone(cond, "draft-release job needs an if: gate")
            self.assertRegex(cond, r"github\.ref_type\s*==\s*['\"]tag['\"]|startsWith\(\s*github\.ref,\s*['\"]refs/tags/")
            self.assertNotRegex(cond, r"==\s*['\"]workflow_dispatch['\"]")
            self.assertNotRegex(cond, r"==\s*['\"]pull_request['\"]")
            self.assertNotRegex(self.jobs[j], r"(?m)^\s*if:.*workflow_dispatch'\s*\|\|")


if __name__ == "__main__":
    unittest.main()
