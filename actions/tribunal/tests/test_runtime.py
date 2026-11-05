"""Offline tests for immutable version resolution and artifact provenance."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("runtime", Path(__file__).resolve().parents[1] / "runtime.py")
runtime = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runtime)


# What `tribunal post` really prints: the PR URL from `gh pr view --json url`, with no review anchor.
ENGINE_POST_LOG = "Posted COMMENT: https://github.com/acme/web-app/pull/12\n"


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.job = Path(self.temp.name) / "tribunal"
        (self.job / "data").mkdir(parents=True)
        self.pr = {"number": 12, "base": {"ref": "main", "sha": "b" * 40}, "head": {"sha": "a" * 40, "ref": "feature/widgets"}}
        self.env = patch.dict(os.environ, {"RUNNER_TEMP": self.temp.name, "GITHUB_REPOSITORY": "acme/web-app", "GITHUB_RUN_ID": "8", "GITHUB_WORKFLOW_REF": "acme/web-app/.github/workflows/tribunal.yml@refs/pull/12/merge"}, clear=True)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.event = patch.object(runtime, "event", return_value=self.pr)
        self.event.start()
        self.addCleanup(self.event.stop)
        runtime.write(self.job / "selection.json", {"version": "115.2.0", "commit": "c" * 40, "binary": "tribunal-rust-v115.2.0"})
        runtime.write(self.job / "profile-hashes.json", {"profile": "profile-hash", "rules": "rules-hash"})
        self.row = {"schemaVersion": 3, "repo": "acme/web-app", "pr": 12, "headSha": "d" * 40, "posted": True, "ts": "2026-11-05T12:00:00Z", "findings": [{"id": "F1", "severity": "important", "body": "Check this finding"}]}
        self.review = {"id": 99, "commit_id": "d" * 40, "state": "COMMENTED", "user": {"login": "github-actions[bot]"}, "body": "Review\n<sub>tribunal v115.2.0</sub>"}
        self.payload = {"schemaVersion": 1, "identity": runtime.identity(self.job), "workflow": "acme/web-app/.github/workflows/tribunal.yml", "run": 7, "review": 99, "row": self.row}
        self.artifact = {"id": 20, "name": "tribunal-baseline-pr-12-7-1", "expired": False, "workflow_run": {"id": 7}}
        self.run_artifacts = {8: [], 7: [self.artifact]}
        self.reviews = [self.review]
        self.source_run = {"workflow_id": 10, "path": ".github/workflows/tribunal.yml", "event": "pull_request", "head_repository": {"full_name": "acme/web-app"}, "pull_requests": [{"number": 12}]}

    def archive(self, payload=None, extra=False):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as archive:
            archive.writestr("baseline.json", json.dumps(payload or self.payload))
            if extra:
                archive.writestr("auth.json", "secret")
        return buf.getvalue()

    def api(self, path):
        return {"repos/acme/web-app/actions/runs/8": {"workflow_id": 10}, "repos/acme/web-app/actions/runs/7": self.source_run, "repos/acme/web-app/pulls/12/reviews/99": self.review, "user": {"login": "dev"}}[path]

    def pages(self, path, key=None):
        if path.startswith("repos/acme/web-app/actions/workflows/10/runs?"):
            self.assertIn("branch=feature%2Fwidgets", path)
            self.assertEqual("workflow_runs", key)
            return [{"id": run} for run in self.run_artifacts]
        if path.startswith("repos/acme/web-app/actions/runs/"):
            self.assertEqual("artifacts", key)
            return self.run_artifacts[int(path.split("/")[5])]
        if path.startswith("repos/acme/web-app/pulls/12/reviews"):
            return self.reviews
        raise AssertionError("unexpected listing " + path)

    def restore(self, archive=None):
        with patch.object(runtime, "api_json", side_effect=self.api), patch.object(runtime, "pages", side_effect=self.pages), patch.object(runtime, "api", return_value=archive or self.archive()):
            runtime.restore_baseline(self.job)

    def save(self):
        with patch.object(runtime, "api_json", side_effect=self.api), patch.object(runtime, "pages", side_effect=self.pages):
            runtime.save_baseline(self.job)

    def test_latest_ignores_other_tools_legacy_and_prereleases(self):
        refs = [{"ref": "refs/tags/" + x} for x in ("tribunal/v115", "tribunal/v115.1.0", "tribunal/v115.2.0", "tribunal/v115.10.0", "tribunal/v116.0.0-rc.1", "tribunal-rust-v999.0.0", "other/v999.0.0")]
        self.assertEqual("115.10.0", runtime.select_latest(refs))

    def test_latest_requires_supported_freeze(self):
        with self.assertRaises(ValueError):
            runtime.select_latest([{"ref": "refs/tags/tribunal/v114.0.0"}])

    def test_exact_version_cannot_be_shell_argument(self):
        for version in ("115.2.0;echo x", "../main", "115.2.0-rc.1", "v115.2.0", "115.02.0"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                runtime.semver(version)

    def test_annotated_tag_is_peeled_once_to_commit(self):
        with patch.object(runtime, "api_json", return_value={"object": {"type": "commit", "sha": "e" * 40}}) as api:
            self.assertEqual("e" * 40, runtime.peel({"type": "tag", "sha": "f" * 40}))
            api.assert_called_once()

    def test_tag_cycle_is_rejected(self):
        with patch.object(runtime, "api_json", return_value={"object": {"type": "tag", "sha": "f" * 40}}), self.assertRaises(ValueError):
            runtime.peel({"type": "tag", "sha": "f" * 40})

    def test_archive_rejects_symlinks_and_traversal(self):
        for name, kind in (("repo/plugins/tribunal/../../escape", tarfile.REGTYPE), ("repo/plugins/tribunal/link", tarfile.SYMTYPE)):
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode="w:gz") as archive:
                item = tarfile.TarInfo(name)
                item.type = kind
                archive.addfile(item, io.BytesIO())
            with self.subTest(name=name), self.assertRaises(ValueError):
                runtime.unpack_plugin(buf.getvalue(), self.job / "source")

    def test_latest_manifest_must_match_selected_tag(self):
        with patch.dict(os.environ, {"INPUT_TRIBUNAL_VERSION": "latest"}), patch.object(runtime, "pages", return_value=[{"ref": "refs/tags/tribunal/v115.3.0"}]), patch.object(runtime, "api_json", return_value={"object": {"type": "commit", "sha": "e" * 40}}), patch.object(runtime, "api", return_value=b"archive"), patch.object(runtime, "unpack_plugin"), patch.object(runtime, "read", return_value={"version": "115.2.0"}):
            with self.assertRaisesRegex(ValueError, "requested plugin version"):
                runtime.resolve(self.job, Path(__file__).resolve().parents[1])

    def plugin_archive(self, version="115.3.0"):
        files = {
            "plugins/tribunal/.claude-plugin/plugin.json": json.dumps({"version": version}),
            "plugins/tribunal/rust/release.json": json.dumps({"tag": "tribunal-rust-v" + version}),
            "plugins/tribunal/agents/lens.md": "Lens from selected commit",
            "plugins/tribunal/methodology/review.md": "Methodology from selected commit",
            "plugins/homosapiens/skills/homosapiens/SKILL.md": "Homosapiens from selected commit",
        }
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as archive:
            for name, body in files.items():
                item = tarfile.TarInfo("snapshot/" + name)
                data = body.encode()
                item.size = len(data)
                archive.addfile(item, io.BytesIO(data))
        return buf.getvalue()

    def test_exact_and_latest_archives_bind_every_asset_to_commit(self):
        for requested in ("115.3.0", "latest"):
            with self.subTest(requested=requested):
                action = Path(self.temp.name) / "no-git/actions/tribunal"
                archive = self.plugin_archive()
                with patch.dict(os.environ, {"INPUT_TRIBUNAL_VERSION": requested, "ACTION_REF": "tribunal-action-v1"}), patch.object(runtime, "pages", return_value=[{"ref": "refs/tags/tribunal/v115.3.0"}]), patch.object(runtime, "api_json", return_value={"sha": "e" * 40, "object": {"type": "commit", "sha": "e" * 40}}), patch.object(runtime, "api", return_value=archive) as api:
                    runtime.resolve(self.job, action)
                api.assert_called_once_with("repos/xmartlabs/agent-skills/tarball/" + "e" * 40, binary=True)
                selection = runtime.read(self.job / "selection.json")
                self.assertEqual("115.3.0", selection["version"])
                self.assertEqual("e" * 40, selection["commit"])
                self.assertEqual("tribunal-rust-v115.3.0", selection["binary"])
                self.assertIn("selected commit", (Path(selection["plugin"]) / "agents/lens.md").read_text())
                self.assertIn("selected commit", (Path(selection["homosapiens"]) / "SKILL.md").read_text())

    def bundled_checkout(self, root):
        for name, body in {
            "plugins/tribunal/.claude-plugin/plugin.json": json.dumps({"version": "115.2.0"}),
            "plugins/tribunal/rust/release.json": json.dumps({"tag": "tribunal-rust-v115.2.0"}),
            "plugins/tribunal/agents/lens.md": "Local lens",
            "plugins/tribunal/methodology/review.md": "Local methodology",
            "plugins/homosapiens/skills/homosapiens/SKILL.md": "Local homosapiens",
        }.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(body)
        return root / "actions/tribunal"

    def test_bundled_uses_local_files_without_network(self):
        for layout, commit in (("_actions/acme/agent-skills/tribunal-action-v1", "ref:tribunal-action-v1"), ("_actions/acme/agent-skills/" + "e" * 40, "e" * 40), ("plain-copy", "unknown")):
            with self.subTest(layout=layout):
                action = self.bundled_checkout(Path(self.temp.name) / "work" / layout)
                with patch.dict(os.environ, {"INPUT_TRIBUNAL_VERSION": "bundled"}), patch.object(runtime, "api", side_effect=AssertionError("network")), patch.object(runtime, "api_json", side_effect=AssertionError("network")):
                    runtime.resolve(self.job, action)
                selection = runtime.read(self.job / "selection.json")
                self.assertEqual(commit, selection["commit"])
                self.assertEqual("tribunal-rust-v115.2.0", selection["binary"])
                self.assertEqual(str(action.parent.parent / "plugins/tribunal"), selection["plugin"])

    def test_reviewer_is_only_checked_when_the_template_is_used(self):
        with patch.dict(os.environ, {"INPUT_REVIEWER": "mobile"}):
            runtime.validate()
            profile = subprocess.CompletedProcess([], 0, stdout=b"{}", stderr=b"")
            with patch.object(runtime.subprocess, "run", return_value=profile):
                runtime.profile(self.job, Path("unused-plugin"))
            missing = subprocess.CompletedProcess([], 1, stdout=b"", stderr=b"gh: Not Found (HTTP 404)")
            with patch.object(runtime.subprocess, "run", return_value=missing), self.assertRaisesRegex(ValueError, "reviewer must be one of"):
                runtime.profile(self.job, Path("unused-plugin"))

    def test_missing_project_rules_names_the_file(self):
        profile = subprocess.CompletedProcess([], 0, stdout=json.dumps({"projectRules": ".claude/review-rules.md"}).encode(), stderr=b"")
        missing = subprocess.CompletedProcess([], 1, stdout=b"", stderr=b"gh: Not Found (HTTP 404)")
        with patch.object(runtime.subprocess, "run", side_effect=[profile, missing]), self.assertRaisesRegex(ValueError, r"\.claude/review-rules\.md"):
            runtime.profile(self.job, Path("unused-plugin"))

    def test_unexpected_errors_print_one_line(self):
        script = Path(__file__).resolve().parents[1] / "runtime.py"
        (self.job / "run.json").write_text("[]")  # .get on a list: an AttributeError, not a ValueError
        env = dict(os.environ, RUNNER_TEMP=self.temp.name)
        result = subprocess.run([sys.executable, str(script), "check-head"], env=env, capture_output=True, text=True)
        self.assertEqual(1, result.returncode)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual(1, len(result.stderr.strip().splitlines()))

    def test_missing_freeze_never_falls_back_to_bundled(self):
        with patch.dict(os.environ, {"INPUT_TRIBUNAL_VERSION": "115.3.0"}), patch.object(runtime, "api_json", side_effect=ValueError("missing freeze")), self.assertRaisesRegex(ValueError, "missing freeze"):
            runtime.resolve(self.job, Path(__file__).resolve().parents[1])

    def test_clean_posted_row_is_saved_but_not_reused(self):
        self.row["headSha"] = self.pr["head"]["sha"]
        self.row["findings"] = []
        self.review["commit_id"] = self.row["headSha"]
        (self.job / "post.log").write_text(ENGINE_POST_LOG)
        (self.job / "data/metrics.jsonl").write_text(json.dumps(self.row) + "\n")
        self.save()
        payload = runtime.read(self.job / "baseline/baseline.json")
        self.assertEqual([], payload["row"]["findings"])
        payload["run"] = 7
        self.pr["head"]["sha"] = "f" * 40
        with self.assertRaisesRegex(ValueError, "no findings"):
            self.restore(self.archive(payload))

    def test_empty_configured_rules_are_snapshotted_and_passed(self):
        profile = json.dumps({"projectRules": ".claude/review-rules.md"}).encode()
        response = subprocess.CompletedProcess([], 0, stdout=profile, stderr=b"")
        empty = subprocess.CompletedProcess([], 0, stdout=b"", stderr=b"")
        with patch.object(runtime.subprocess, "run", side_effect=[response, empty]):
            runtime.profile(self.job, Path("unused-plugin"))
        rules = self.job / "review-rules.md"
        self.assertTrue(rules.is_file(), "even empty rules must be materialized")
        self.assertEqual(b"", rules.read_bytes())
        self.assertIn("--rules\n" + str(rules), (self.job / "profile-args").read_text())
        self.assertEqual(runtime.digest(b""), runtime.read(self.job / "profile-hashes.json")["rules"])

    def test_structured_arguments_keep_values_separate(self):
        with patch.dict(os.environ, {"INPUT_MODEL": "claude-model", "INPUT_EFFORT": "high", "INPUT_CONCURRENCY": "2", "INPUT_BOT_COMMENTS": "true"}):
            self.assertEqual(["--model", "claude-model", "--effort", "high", "--concurrency", "2", "--no-codex", "--bot-comments"], runtime.review_args())

    def test_verified_baseline_is_restored(self):
        self.restore()
        self.assertEqual(self.row, json.loads((self.job / "data/metrics.jsonl").read_text()))

    def test_restore_rejects_incompatible_identity(self):
        for field, bad in (("repository", "other/repo"), ("pr", 13), ("version", "115.3.0"), ("base", "wrong"), ("hashes", {}), ("pluginCommit", "wrong")):
            payload = copy.deepcopy(self.payload)
            payload["identity"][field] = bad
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.restore(self.archive(payload))

    def test_restore_rejects_untrusted_source_workflow(self):
        for field, bad in (("workflow_id", 11), ("event", "push"), ("head_repository", {"full_name": "fork/web-app"}), ("pull_requests", [{"number": 13}])):
            original = self.source_run[field]
            self.source_run[field] = bad
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.restore()
            self.source_run[field] = original

    def test_restore_rejects_unpublished_wrong_head_wrong_author_or_empty_findings(self):
        for field, bad in (("posted", False), ("findings", []), ("schemaVersion", 2), ("headSha", "wrong")):
            payload = copy.deepcopy(self.payload)
            payload["row"][field] = bad
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.restore(self.archive(payload))
        self.review["user"]["login"] = "someone-else"
        with self.assertRaises(ValueError):
            self.restore()

    def test_restore_rejects_missing_expired_or_current_artifact(self):
        for changes in ({"expired": True}, {"name": "another-artifact"}):
            original = dict(self.artifact)
            self.artifact.update(changes)
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.restore()
            self.artifact.clear()
            self.artifact.update(original)
        current = dict(self.artifact, name="tribunal-baseline-pr-12-8-1", workflow_run={"id": 8})
        self.run_artifacts = {8: [current], 7: []}
        with self.assertRaisesRegex(ValueError, "no prior baseline"):
            self.restore()

    def test_restore_takes_newest_run_with_a_baseline(self):
        self.run_artifacts = {8: [], 9: [], 7: [self.artifact], 6: [dict(self.artifact, id=5, name="tribunal-baseline-pr-12-6-1", workflow_run={"id": 6})]}
        self.restore()
        self.assertEqual(self.row, json.loads((self.job / "data/metrics.jsonl").read_text()))

    def test_restore_rejects_deleted_head_repository_without_traceback(self):
        self.source_run["head_repository"] = None
        with self.assertRaisesRegex(ValueError, "another workflow"):
            self.restore()

    def test_restore_rejects_extra_files_and_malformed_archive(self):
        for archive in (self.archive(extra=True), b"not-a-zip"):
            with self.subTest(archive=archive[:10]), self.assertRaises((ValueError, zipfile.BadZipFile)):
                self.restore(archive)

    def test_save_keeps_only_posted_metrics_and_provenance(self):
        self.row["headSha"] = self.pr["head"]["sha"]
        self.row["secret"] = "not-for-artifact"
        self.review["commit_id"] = self.row["headSha"]
        (self.job / "post.log").write_text(ENGINE_POST_LOG)
        (self.job / "data/metrics.jsonl").write_text(json.dumps(self.row) + "\n")
        self.save()
        saved = runtime.read(self.job / "baseline/baseline.json")
        self.assertNotIn("secret", saved["row"])
        self.assertEqual(99, saved["review"])

    def test_save_finds_review_from_real_engine_output(self):
        self.row["headSha"] = self.pr["head"]["sha"]
        self.review["commit_id"] = self.row["headSha"]
        older = dict(self.review, id=50)
        human = dict(self.review, id=120, user={"login": "someone-else"})
        stale = dict(self.review, id=110, commit_id="f" * 40)
        self.reviews = [older, self.review, stale, human]
        (self.job / "post.log").write_text(ENGINE_POST_LOG)
        (self.job / "data/metrics.jsonl").write_text(json.dumps(self.row) + "\n")
        self.save()
        self.assertEqual(99, runtime.read(self.job / "baseline/baseline.json")["review"])

    def test_save_refuses_when_no_own_review_matches(self):
        self.row["headSha"] = self.pr["head"]["sha"]
        self.reviews = [dict(self.review, user={"login": "someone-else"}, commit_id=self.row["headSha"])]
        (self.job / "post.log").write_text(ENGINE_POST_LOG)
        (self.job / "data/metrics.jsonl").write_text(json.dumps(self.row) + "\n")
        with self.assertRaisesRegex(ValueError, "published review"):
            self.save()

    def test_save_refuses_unposted_metrics(self):
        self.row["posted"] = False
        (self.job / "post.log").write_text(ENGINE_POST_LOG)
        (self.job / "data/metrics.jsonl").write_text(json.dumps(self.row) + "\n")
        with self.assertRaises(ValueError):
            runtime.save_baseline(self.job)


if __name__ == "__main__":
    unittest.main()
