#!/usr/bin/env python3
"""Trusted CI adapter: configuration, immutable releases, and baseline provenance.

This is setup/bookkeeping, not review logic. All subprocess arguments are arrays;
repository-controlled strings are never evaluated as shell programs.
"""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import urllib.parse
import zipfile

SOURCE_REPO = "xmartlabs/agent-skills"
MIN_VERSION = (115, 2, 0)
# The first engine that keeps conventions-token out of the reviewers' environment.
MIN_CONVENTIONS_TOKEN_VERSION = (115, 4, 0)
REVIEWERS = ("frontend", "backend", "python", "e2e")
RESTORE_RUN_LIMIT = 30  # newest runs of this workflow on the PR branch to search for a baseline
NUMBERS = ("concurrency", "max-panels", "max-agents", "max-output-tokens", "prior-verify", "lens-samples")


def setting(name, default=""):
    return os.environ.get("INPUT_" + name.upper().replace("-", "_"), default)


def semver(value):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("Tribunal version must be bundled, latest, or a stable x.y.z version")
    version = tuple(map(int, value.split(".")))
    if version < MIN_VERSION:
        raise ValueError("Tribunal versions before 115.2.0 are not supported by this action")
    return version


def validate():
    version = setting("tribunal-version", "bundled")
    if version not in ("bundled", "latest"):
        semver(version)
    for name, choices, default in (
        ("verdict", ("comment", "suggested"), "comment"),
        ("review-mode", ("full", "incremental"), "full"),
        ("codex", ("true", "false"), "false"),
        ("bot-comments", ("true", "false"), "false"),
        ("effort", ("", "low", "medium", "high", "max"), ""),
    ):
        if setting(name, default) not in choices:
            raise ValueError(f"{name} must be one of {', '.join(repr(c) for c in choices)}")
    for name in NUMBERS:
        value = setting(name)
        if value and (not re.fullmatch(r"[0-9]+", value) or not 1 <= int(value) <= (4 if name == "lens-samples" else 2**31 - 1)):
            raise ValueError(f"{name} must be a positive whole number" + (" from 1 to 4" if name == "lens-samples" else ""))
    for name in ("model", "codex-model"):
        value = setting(name)
        if value and not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]*", value):
            raise ValueError(f"{name} must be a model identifier, not command-line arguments")
    if setting("codex", "false") == "true" and not setting("openai-api-key"):
        raise ValueError("codex: true requires openai-api-key")


def review_args():
    args = []
    for name in ("model", "effort") + NUMBERS:
        if setting(name):
            args += ["--" + name, setting(name)]
    args.append("--codex" if setting("codex", "false") == "true" else "--no-codex")
    if setting("bot-comments", "false") == "true":
        args.append("--bot-comments")
    return args


def call(*args, binary=False):
    result = subprocess.run(args, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        # Do not replay arbitrary tool output: it may contain credentials or PR text.
        raise ValueError(f"{args[0]} request failed (exit {result.returncode})")
    return result.stdout if binary else result.stdout.decode()


def api(path, binary=False):
    return call("gh", "api", path, binary=binary)


def api_json(path):
    return json.loads(api(path))


def pages(path, key=None):
    pages_ = json.loads(call("gh", "api", "--paginate", "--slurp", path))
    return [item for page in pages_ for item in (page[key] if key else page)]


def write(path, value):
    Path(path).write_text(json.dumps(value, sort_keys=True) + "\n")


def read(path):
    return json.loads(Path(path).read_text())


def digest(data):
    return hashlib.sha256(data).hexdigest()


def select_latest(refs):
    candidates = []
    for ref in refs:
        value = ref.get("ref", "").removeprefix("refs/tags/tribunal/v")
        try:
            candidates.append((semver(value), value))
        except ValueError:
            pass
    if not candidates:
        raise ValueError("no supported stable Tribunal freeze was found")
    return max(candidates)[1]


def peel(obj):
    for _ in range(10):
        if obj.get("type") == "commit" and re.fullmatch(r"[0-9a-f]{40}", obj.get("sha", "")):
            return obj["sha"]
        if obj.get("type") != "tag":
            break
        obj = api_json(f"repos/{SOURCE_REPO}/git/tags/{obj['sha']}")["object"]
    raise ValueError("Tribunal freeze does not resolve to a commit")


def unpack_plugin(data, destination):
    """Extract only regular files in the two trusted plugin subtrees; reject links."""
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            parts = PurePosixPath(member.name).parts
            if len(parts) < 4 or parts[1:3] not in (("plugins", "tribunal"), ("plugins", "homosapiens")):
                continue
            rel = PurePosixPath(*parts[1:])
            if ".." in rel.parts or rel.is_absolute() or not (member.isfile() or member.isdir()):
                raise ValueError("unsafe entry in plugin archive")
            path = destination.joinpath(*rel.parts)
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.extractfile(member).read())


def bundled_commit(root):
    """Best-effort label for the bundled checkout: a SHA, `ref:<ref>`, or `unknown`."""
    if (root / ".git").exists():
        return call("git", "-C", str(root), "rev-parse", "HEAD").strip()
    # Runners unpack `uses: owner/repo/path@ref` into .../_actions/<owner>/<repo>/<ref>/.
    if root.parent.parent.parent.name == "_actions":
        return root.name if re.fullmatch(r"[0-9a-f]{40}", root.name) else "ref:" + root.name
    return "unknown"


def resolve(job, action):
    requested = setting("tribunal-version", "bundled")
    root = action.parent.parent
    selected_version = None
    if requested == "bundled":
        # The plugin files ship in the same checkout as this script, so they are
        # already from the wrapper's commit; no download, works on forks and GHES.
        plugin = root / "plugins/tribunal"
        homosapiens = root / "plugins/homosapiens/skills/homosapiens"
        sha = bundled_commit(root)
    else:
        version = select_latest(pages(f"repos/{SOURCE_REPO}/git/matching-refs/tags/tribunal/v")) if requested == "latest" else requested
        semver(version)
        selected_version = version
        obj = api_json(f"repos/{SOURCE_REPO}/git/ref/tags/tribunal/v{version}")["object"]
        sha = peel(obj)
        source = job / "source"
        unpack_plugin(api(f"repos/{SOURCE_REPO}/tarball/{sha}", binary=True), source)
        plugin = source / "plugins/tribunal"
        homosapiens = source / "plugins/homosapiens/skills/homosapiens"
    manifest = read(plugin / ".claude-plugin/plugin.json")
    version = manifest["version"]
    if os.environ.get("HAS_CONVENTIONS_TOKEN") == "true" and semver(version) < MIN_CONVENTIONS_TOKEN_VERSION:
        raise ValueError(f"conventions-token needs Tribunal 115.4.0 or newer, and this run selected {version}; an older engine would ignore it and pass it to the reviewers")
    semver(version)
    if selected_version is not None and version != selected_version:
        raise ValueError("selected freeze does not contain the requested plugin version")
    pin = read(plugin / "rust/release.json")
    if not (plugin / "agents").is_dir() or not (plugin / "methodology").is_dir() or not (homosapiens / "SKILL.md").is_file():
        raise ValueError("selected freeze lacks required Tribunal/homosapiens files")
    write(job / "selection.json", {"version": version, "commit": sha, "binary": pin["tag"], "plugin": str(plugin), "homosapiens": str(homosapiens)})
    (job / "plugin-path").write_text(str(plugin))
    (job / "homosapiens-path").write_text(str(homosapiens))


def event():
    return read(os.environ["GITHUB_EVENT_PATH"])["pull_request"]


def profile(job, plugin):
    pr = event()
    repo = os.environ["GITHUB_REPOSITORY"]
    base = pr["base"].get("sha") or pr["base"]["ref"]
    head = pr["head"]["sha"]
    raw = None
    selected = None
    for ref in (base, None, head):
        query = "" if ref is None else "?ref=" + urllib.parse.quote(ref, safe="")
        path = f"repos/{repo}/contents/.claude/review-profile.json{query}"
        result = subprocess.run(["gh", "api", "-H", "Accept: application/vnd.github.raw", path], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if result.returncode == 0:
            raw, selected = result.stdout, query
            break
        if b"HTTP 404" not in result.stderr:
            raise ValueError("could not load the repository review profile")
    if raw is None:
        reviewer = setting("reviewer", "frontend")
        if reviewer not in REVIEWERS:
            raise ValueError(f"the repository has no .claude/review-profile.json and reviewer must be one of {', '.join(repr(r) for r in REVIEWERS)}")
        data = read(plugin / f"templates/review-profile.{reviewer}.json")
        data.update(project=repo.split("/")[-1], repo=repo, baseBranch=pr["base"]["ref"], crossRepoAnchors=[])
        data.pop("projectRules", None)
        raw = json.dumps(data, sort_keys=True).encode()
    data = json.loads(raw)
    rules = b""
    rule_path = data.get("projectRules")
    if rule_path:
        if not isinstance(rule_path, str) or PurePosixPath(rule_path).is_absolute() or ".." in PurePosixPath(rule_path).parts:
            raise ValueError("projectRules must be a repository-relative path")
        result = subprocess.run(["gh", "api", "-H", "Accept: application/vnd.github.raw", f"repos/{repo}/contents/{urllib.parse.quote(rule_path, safe='/')}{selected}"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if result.returncode != 0:
            where = f"at {selected[len('?ref='):]}" if selected else "on the default branch"
            if b"HTTP 404" in result.stderr:
                raise ValueError(f"projectRules file {rule_path} was not found {where}")
            raise ValueError(f"could not load projectRules file {rule_path} {where}")
        rules = result.stdout
    (job / "review-profile.json").write_bytes(raw)
    args = ["--profile", str(job / "review-profile.json")]
    if rule_path:
        (job / "review-rules.md").write_bytes(rules)
        args += ["--rules", str(job / "review-rules.md")]
    (job / "profile-args").write_text("\n".join(args) + "\n")
    write(job / "profile-hashes.json", {"profile": digest(raw), "rules": digest(rules)})


def identity(job):
    pr = event()
    selection = read(job / "selection.json")
    return {"repository": os.environ["GITHUB_REPOSITORY"], "pr": pr["number"], "base": pr["base"].get("sha", ""),
            "version": selection["version"], "pluginCommit": selection["commit"], "binary": selection["binary"],
            "hashes": read(job / "profile-hashes.json")}


def current_workflow():
    repo = os.environ["GITHUB_REPOSITORY"]
    run = api_json(f"repos/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']}")
    if not isinstance(run.get("workflow_id"), int):
        raise ValueError("current workflow identity is unavailable")
    return run["workflow_id"]


def workflow_path():
    value = os.environ.get("GITHUB_WORKFLOW_REF", "").split("@", 1)[0]
    prefix = os.environ["GITHUB_REPOSITORY"] + "/.github/workflows/"
    if not value.startswith(prefix) or not re.fullmatch(r"[A-Za-z0-9_.-]+\.ya?ml", value[len(prefix):]):
        raise ValueError("workflow file identity is unavailable")
    return value


def check_review(row, review, allowed, require_findings=True):
    return (row.get("posted") is True and row.get("schemaVersion", 0) >= 3
            and (not require_findings or bool(row.get("findings"))) and isinstance(row.get("findings"), list)
            and review.get("commit_id") == row.get("headSha")
            and review.get("user", {}).get("login") in allowed
            and review.get("state") in ("COMMENTED", "APPROVED", "CHANGES_REQUESTED")
            and "<sub>tribunal v" in review.get("body", ""))


def own_logins():
    logins = ["github-actions[bot]"]
    try:
        login = api_json("user").get("login")
        if login:
            logins.append(login)
    except (ValueError, KeyError):
        pass
    return logins


def baseline_payload(job, row, review, workflow):
    expected = identity(job)
    if not expected["base"]:
        raise ValueError("base commit is unavailable")
    if row.get("repo") != expected["repository"] or row.get("pr") != expected["pr"] or not check_review(row, review, own_logins(), require_findings=False):
        raise ValueError("posted review provenance could not be verified")
    safe_row = {key: row[key] for key in ("schemaVersion", "repo", "pr", "headSha", "findings", "posted", "ts") if key in row}
    return {"schemaVersion": 1, "identity": expected, "workflow": workflow, "run": int(os.environ["GITHUB_RUN_ID"]), "review": review["id"], "row": safe_row}


def save_baseline(job):
    if not re.search(r"^Posted [A-Za-z _-]+: ", (job / "post.log").read_text(), re.M):
        raise ValueError("the engine did not report a published review")
    ledger = [json.loads(line) for line in (job / "data/metrics.jsonl").read_text().splitlines() if line.strip()]
    head = event()["head"]["sha"]
    row = next((r for r in reversed(ledger) if r.get("headSha") == head and r.get("posted") is True and r.get("pr") == event()["number"] and r.get("repo") == os.environ["GITHUB_REPOSITORY"]), None)
    if row is None:
        raise ValueError("no posted metrics row for this run")
    # `tribunal post` prints the PR URL, not the review's, so find the review it
    # just published: ours, with the Tribunal footer, on this exact head.
    allowed = own_logins()
    reviews = pages(f"repos/{os.environ['GITHUB_REPOSITORY']}/pulls/{event()['number']}/reviews?per_page=100")
    matches = [r for r in reviews if isinstance(r.get("id"), int) and check_review(row, r, allowed, require_findings=False)]
    if not matches:
        raise ValueError("the published review could not be identified")
    review = max(matches, key=lambda r: r["id"])
    payload = baseline_payload(job, row, review, workflow_path())
    (job / "baseline").mkdir(exist_ok=True)
    write(job / "baseline/baseline.json", payload)


def restore_baseline(job):
    expected = identity(job)
    if not expected["base"]:
        raise ValueError("base commit is unavailable")
    repo, pr = expected["repository"], expected["pr"]
    workflow_id = current_workflow()
    workflow = workflow_path()
    name = re.compile(rf"tribunal-baseline-pr-{pr}-([0-9]+)-([0-9]+)$")
    branch = urllib.parse.quote(event()["head"]["ref"], safe="")
    runs = pages(f"repos/{repo}/actions/workflows/{workflow_id}/runs?event=pull_request&branch={branch}&per_page=100", "workflow_runs")
    current = int(os.environ["GITHUB_RUN_ID"])
    artifact = None
    for source in sorted((r["id"] for r in runs if isinstance(r.get("id"), int) and r["id"] != current), reverse=True)[:RESTORE_RUN_LIMIT]:
        candidates = [a for a in pages(f"repos/{repo}/actions/runs/{source}/artifacts?per_page=100", "artifacts")
                      if name.fullmatch(a.get("name", "")) and (a.get("workflow_run") or {}).get("id") == source]
        if candidates:
            artifact = max(candidates, key=lambda a: a["id"])
            break
    if artifact is None:
        raise ValueError("no prior baseline artifact")
    if artifact.get("expired"):
        raise ValueError("the latest baseline artifact has expired")
    run_id = artifact["workflow_run"]["id"]
    run = api_json(f"repos/{repo}/actions/runs/{run_id}")
    if (run.get("workflow_id") != workflow_id or run.get("path", "").split("@", 1)[0] != workflow.split("/", 2)[2] or run.get("event") != "pull_request"
            or (run.get("head_repository") or {}).get("full_name") != repo
            or not any(p.get("number") == pr for p in run.get("pull_requests") or [])
            or name.fullmatch(artifact["name"])[1] != str(run_id)):
        raise ValueError("baseline artifact belongs to another workflow or pull request")
    archive = api(f"repos/{repo}/actions/artifacts/{artifact['id']}/zip", binary=True)
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        if zipped.namelist() != ["baseline.json"] or zipped.getinfo("baseline.json").file_size > 10_000_000:
            raise ValueError("invalid baseline archive")
        payload = json.loads(zipped.read("baseline.json"))
    if (payload.get("schemaVersion") != 1 or payload.get("identity") != expected or payload.get("workflow") != workflow or payload.get("run") != run_id):
        raise ValueError("baseline is incompatible with this run")
    row = payload.get("row", {})
    if row.get("repo") != repo or row.get("pr") != pr or row.get("headSha") == event()["head"]["sha"]:
        raise ValueError("baseline metrics do not describe a previous head of this PR")
    review_id = payload.get("review")
    if not isinstance(review_id, int) or review_id <= 0:
        raise ValueError("baseline has no review identity")
    review = api_json(f"repos/{repo}/pulls/{pr}/reviews/{review_id}")
    if not check_review(row, review, own_logins()):
        raise ValueError("baseline review is unverified or carries no findings")
    (job / "data/metrics.jsonl").write_text(json.dumps(row) + "\n")


def main():
    command = sys.argv[1]
    job = Path(os.environ["RUNNER_TEMP"]) / "tribunal"
    if command == "validate":
        validate()
    elif command == "args":
        print("\n".join(review_args()))
    elif command == "resolve":
        resolve(job, Path(__file__).resolve().parent)
    elif command == "profile":
        profile(job, Path((job / "plugin-path").read_text()))
    elif command == "restore":
        restore_baseline(job)
    elif command == "save":
        save_baseline(job)
    elif command == "check-head":
        run = read(job / "run.json")
        if run.get("skipped"):
            return
        if run.get("metrics", {}).get("headSha") != event()["head"]["sha"]:
            raise ValueError("reviewed HEAD differs from the triggering event; refusing to publish")
    else:
        raise ValueError("unknown runtime command")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, TypeError, zipfile.BadZipFile, tarfile.TarError) as error:
        print(f"tribunal: {error}", file=sys.stderr)
        sys.exit(1)
    except Exception as error:  # one line for the log and job summary, never a traceback
        print(f"tribunal: unexpected {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(1)
