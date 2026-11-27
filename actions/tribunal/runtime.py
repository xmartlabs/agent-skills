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
# The first engine that writes `needsHuman` into run.json.
MIN_NEEDS_HUMAN_VERSION = (115, 9, 0)
REVIEWERS = ("frontend", "backend", "python", "e2e")
RESTORE_RUN_LIMIT = 30  # newest runs of this workflow on the PR branch to search for a baseline
NUMBERS = ("concurrency", "max-panels", "max-agents", "max-output-tokens", "prior-verify", "lens-samples")
CHECK_NAME = "Tribunal findings"
HUMAN_CHECK_NAME = "Tribunal human review"
HUMAN_SKIP_MODES = ("carry", "success", "pending")
NOT_REVIEWED = "Not reviewed by Tribunal"
# The legacy `permission` values that may clear the human review status (maintain reads as write).
WRITE_PERMISSIONS = ("admin", "maintain", "write")
# The engine's `questions::BLOCKING_WORTH`: a finding kept from blocking at this worth or more is
# reported under "Should fix (not blocking)".
SHOULD_FIX_WORTH = 4


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
        ("verdict", ("comment", "suggested", "no-approve"), "comment"),
        ("review-mode", ("full", "incremental"), "full"),
        ("codex", ("true", "false"), "false"),
        ("bot-comments", ("true", "false"), "false"),
        ("findings-check", ("off", "inform", "enforce"), "off"),
        ("human-review-check", ("off", "on"), "off"),
        ("human-review-on-skip", HUMAN_SKIP_MODES, "carry"),
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
    if setting("needs-human-label") and setting("needs-human-label") == setting("label", "tribunal"):
        raise ValueError("needs-human-label must differ from label")


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
    needs_human = [name for name, on in (("needs-human-label", bool(setting("needs-human-label"))), ("human-review-check", setting("human-review-check") == "on")) if on]
    if needs_human and semver(version) < MIN_NEEDS_HUMAN_VERSION:
        names = ", ".join(needs_human)
        raise ValueError(f"{names} {'needs' if len(needs_human) == 1 else 'need'} Tribunal 115.9.0 or later, and this run selected {version}")
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
    # The engine appends this run's row to the same ledger; the findings check needs the prior one.
    write(job / "prior-row.json", row)


def finding_tier(finding):
    """`blocking`, `should-fix` or None: the section of the posted report a kept finding lands in.

    Mirrors the engine's own sort: `blocking` is its final flag, after the evidence gate; a finding
    kept from blocking by the ranking but rated worth 4 or more is "Should fix (not blocking)",
    unless it is about code the pull request did not change.
    """
    if not isinstance(finding, dict) or finding.get("survived") is not True:
        return None
    if finding.get("blocking") is True:
        return "blocking"
    worth = finding.get("rankWorth")
    if (finding.get("rankedBelowBlocking") is True and finding.get("preExistingCode") is not True
            and isinstance(worth, int) and not isinstance(worth, bool) and worth >= SHOULD_FIX_WORTH):
        return "should-fix"
    return None


def open_findings(run, prior_row=None):
    """The open blocking and should-fix findings after this run, each with where its status comes from.

    On an incremental run a prior finding stays open unless this run raised it again (its new tier
    wins) or a verifier found it FIXED; a prior finding on a file this run did not re-read is
    carried forward as it was.
    """
    metrics = run.get("metrics")
    if not isinstance(metrics, dict) or not isinstance(metrics.get("findings"), list):
        raise ValueError("the run has no metrics row to count findings from")

    def item(finding, tier, source):
        return {"tier": tier, "file": finding.get("file"), "line": finding.get("line"),
                "title": finding.get("title"), "source": source}

    def key(finding):
        return (finding.get("file"), finding.get("title"))

    found = [item(f, finding_tier(f), "this review") for f in metrics["findings"] if finding_tier(f)]
    if metrics.get("reviewMode") != "incremental" or not prior_row:
        return found
    raised = {key(f) for f in metrics["findings"] if isinstance(f, dict)}
    checked = {key(p): p.get("verdict") for p in metrics.get("priorVerification") or [] if isinstance(p, dict)}
    for prior in prior_row.get("findings") or []:
        tier = finding_tier(prior)
        if not tier or key(prior) in raised:
            continue
        verdict = checked.get(key(prior))
        if verdict == "FIXED":
            continue
        if verdict == "STILL_OPEN":
            source = "earlier review, re-checked: still open"
        elif verdict:
            source = "earlier review, re-checked: could not tell"
        else:
            source = "earlier review, carried forward without a re-check"
        found.append(item(prior, tier, source))
    return found


def needs_human(job):
    """True when the review raised needsHuman, False when it did not, None when it cannot be told.

    None covers no run.json, an engine skip and an engine older than 115.9.0 (no `needsHuman` key).
    """
    path = job / "run.json"
    if not path.is_file():
        return None
    run = read(path)
    if not isinstance(run, dict) or run.get("skipped") or not isinstance(run.get("needsHuman"), dict):
        return None
    return run["needsHuman"].get("raised") is True


def needs_human_reasons(job):
    reasons = read(job / "run.json")["needsHuman"].get("reasons") or []
    return [f"- {r.get('detail')} ({r.get('signal')})" for r in reasons if isinstance(r, dict)]


def post_state(job):
    """The `tribunal post --verdict` value for the job's `verdict`, or None to post the engine's own.

    `no-approve` posts a change request when the engine asks for one and a comment otherwise, so
    the action never approves however clean the review is. `suggested` posts the engine's own
    verdict, except that an approval becomes a comment while the review needs a human.
    """
    mode = (job / "verdict").read_text().strip()
    if mode == "suggested":
        if needs_human(job) is True and read(job / "run.json").get("verdict") == "APPROVE":
            return "comment"
        return None
    if mode == "no-approve" and read(job / "run.json").get("verdict") == "REQUEST CHANGES":
        return "request-changes"
    return "comment"


def findings_check(job):
    """The commit status for the reviewed head, the table for the job summary, and the two counts.

    A commit status, not a check run: a check run created with the job's token lands in whichever
    workflow's group GitHub picks, while a status shows on the pull request under its own name.
    """
    mode = setting("findings-check", "off")
    run = read(job / "run.json")
    head = event()["head"]["sha"]
    if run.get("metrics", {}).get("headSha") != head:
        raise ValueError("reviewed HEAD differs from the triggering event")
    prior = read(job / "prior-row.json") if (job / "prior-row.json").is_file() else None
    run_url = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    status = {"context": CHECK_NAME, "target_url": run_url}
    heading = f"### {CHECK_NAME}\n\nOpen findings after the tribunal review of `{head[:8]}` ({'enforced' if mode == 'enforce' else 'informative'})."
    try:
        found = open_findings(run, prior)
    except ValueError as error:
        status.update(state="error", description="Open findings could not be counted: read the review")
        return status, f"{heading}\n\n{error}. This is not a clean review: read the review on the pull request.\n", None, None
    blocking = sum(1 for f in found if f["tier"] == "blocking")
    should_fix = sum(1 for f in found if f["tier"] == "should-fix")
    if blocking or should_fix:
        counts = f"{blocking} blocking, {should_fix} should fix open"
        state, description = ("failure", counts) if mode == "enforce" else ("success", f"{counts} (not enforced)")
    else:
        state, description = "success", "No blocking or should-fix findings open"
    lines = [heading, ""]
    if found:
        lines += ["| Tier | Where | Finding | Status from |", "| --- | --- | --- | --- |"]
        for f in found:
            where = f"{f['file']}:{f['line']}" if f.get("line") is not None else str(f["file"])
            title_cell = str(f["title"] or "").replace("|", "\\|").replace("\n", " ")
            lines.append(f"| {'Blocking' if f['tier'] == 'blocking' else 'Should fix'} | `{where}` | {title_cell} | {f['source']} |")
    else:
        lines.append("Nothing open. Nits, questions and pre-existing findings do not count.")
    status.update(state=state, description=description[:140])
    return status, "\n".join(lines) + "\n", blocking, should_fix


def human_review_status(job):
    """The "Tribunal human review" commit status for the reviewed head.

    Pending while the review raised needsHuman, success when it did not. A raised flag is cleared
    at once, as an approval would clear it, when a person who may clear it already approved the
    head (an approval given while the review ran). Raises ValueError when the run cannot say (no
    run.json, an engine skip, an engine older than 115.9.0) or the head moved.
    """
    if not (job / "run.json").is_file():
        raise ValueError("no review ran (no run.json)")
    raised = needs_human(job)
    if raised is None:
        raise ValueError("the engine skipped the review or is older than 115.9.0: it does not say whether a person should look at it")
    run = read(job / "run.json")
    pr = event()
    head = pr["head"]["sha"]
    if run.get("metrics", {}).get("headSha") != head:
        raise ValueError("reviewed HEAD differs from the triggering event")
    repo = os.environ["GITHUB_REPOSITORY"]
    run_url = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    if raised:
        approver = earlier_approver(repo, pr["number"], head)
        if approver:
            state, description = "success", f"Cleared by @{approver}'s approval"
        else:
            signals = ", ".join(str(r.get("signal")) for r in run["needsHuman"].get("reasons") or [] if isinstance(r, dict))
            state, description = "pending", f"A person should review: {signals}"
    else:
        state, description = "success", "No reason for a human review"
    return {"context": HUMAN_CHECK_NAME, "target_url": run_url, "state": state, "description": description[:140]}


def carried_human_review(statuses_by_sha):
    """The state and description `carry` gives a commit Tribunal did not review.

    `statuses_by_sha` is the candidate commits in the order to try them: (sha, statuses) pairs, or
    (sha, statuses, True) for the commit the action's latest review read. Each commit's statuses are
    the latest per context (the combined status). The first candidate with a "Tribunal human
    review" status gives it, a status carried before keeping the commit it first came from. A
    reviewed commit without one gives pending: its status may not be posted yet. Without any, the
    commit gets success.
    """
    for sha, statuses, *reviewed in statuses_by_sha:
        found = next((s for s in statuses if isinstance(s, dict) and s.get("context") == HUMAN_CHECK_NAME), None)
        if found is None or found.get("state") not in ("pending", "success", "failure", "error"):
            if reviewed and reviewed[0]:
                return {"state": "pending", "description": f"Reviewed at {sha[:7]}; human review status not posted yet"}
            continue
        description = str(found.get("description") or "")
        if not re.match(r"Carried from [0-9a-f]{7}: ", description):
            description = f"Carried from {sha[:7]}: {description}"
        return {"state": found["state"], "description": description[:140]}
    return {"state": "success", "description": NOT_REVIEWED}


def pr_reviews(repo, number):
    """The pull request's reviews, oldest first: the listing the gate's `once` reads too."""
    listed = call("gh", "api", "--paginate", f"repos/{repo}/pulls/{number}/reviews?per_page=100", "--jq", ".[] | tojson")
    return [r for r in (json.loads(line) for line in listed.splitlines() if line.strip()) if isinstance(r, dict)]


def latest_own_review_commit(repo, number):
    """The commit the action's latest own review read (the reviews the gate's `once` counts), or None."""
    logins = own_logins()
    commit = None
    for review in pr_reviews(repo, number):
        if ((review.get("user") or {}).get("login") in logins
                and "<sub>tribunal v" in (review.get("body") or "") and review.get("commit_id")):
            commit = review["commit_id"]
    return commit


def combined_statuses(repo, sha):
    """A commit's combined status: the latest status of each context, every page of it.

    GitHub pages the combined status's `statuses` (30 by default, at most 100), so a commit with
    more contexts than one page holds is read page by page until `total_count` are in.
    """
    statuses, page = [], 1
    while True:
        listed = json.loads(api(f"repos/{repo}/commits/{sha}/status?per_page=100&page={page}"))
        if not isinstance(listed, dict) or not isinstance(listed.get("statuses"), list):
            return statuses
        statuses += listed["statuses"]
        total = listed.get("total_count")
        if not listed["statuses"] or not isinstance(total, int) or len(statuses) >= total:
            return statuses
        page += 1


def approval_refusal(repo, review, head, logins):
    """Why `review` cannot clear the human review status of `head`, or None when it can.

    It can when it is an approval of `head` by a user who is not a bot, not one of the action's own
    `logins`, and has write, maintain or admin access. A permission that cannot be read counts as
    no write access.
    """
    user = review.get("user") if isinstance(review.get("user"), dict) else {}
    login, commit, state = user.get("login") or "", review.get("commit_id") or "", str(review.get("state") or "")
    if state.lower() != "approved":
        return f"the review is {state or 'without a state'}, not an approval"
    if not login:
        return "the review has no author"
    if user.get("type") == "Bot":
        return f"@{login} is a bot"
    if not head or commit != head:
        return f"the approval is of {commit[:7] or 'an unknown commit'}, not the head {head[:7]}"
    if login in logins:
        return f"@{login} is the action's own identity"
    try:
        path = f"repos/{repo}/collaborators/{urllib.parse.quote(login, safe='')}/permission"
        permission = call("gh", "api", path, "--jq", ".permission").strip()
    except ValueError:
        return f"could not read @{login}'s permission"
    if permission not in WRITE_PERMISSIONS:
        return f"@{login} has {permission or 'no'} access, not write"
    return None


def earlier_approver(repo, number, head):
    """The login of a person whose approval of `head`, already on the pull request, clears the flag.

    Each user's latest approval, change request or dismissal counts, so an approval its author
    later took back does not. None when there is none, or the reviews cannot be read (the status
    then stays pending, the safe side).
    """
    try:
        reviews = pr_reviews(repo, number)
    except ValueError as error:
        print(f"tribunal: could not list the reviews for an earlier approval: {error}", file=sys.stderr)
        return None
    latest = {}
    for review in reviews:
        login = (review.get("user") or {}).get("login") if isinstance(review.get("user"), dict) else None
        if login and str(review.get("state") or "").upper() in ("APPROVED", "CHANGES_REQUESTED", "DISMISSED"):
            latest[login] = review
    if not latest:
        return None
    logins = own_logins()
    for login, review in latest.items():
        if approval_refusal(repo, review, head, logins) is None:
            return login
    return None


def human_review_approval_status():
    """The "Tribunal human review" status a `pull_request_review` event clears, or why it does not.

    Returns (status, None) when a `submitted` review passes `approval_refusal` while the head's
    newest human review status is pending, else (None, reason). An absent status is not cleared: a
    review that failed must not be cleared blind.
    """
    repo, payload = os.environ["GITHUB_REPOSITORY"], read(os.environ["GITHUB_EVENT_PATH"])
    if payload.get("action") != "submitted":
        return None, f"the review event is {payload.get('action') or 'without an action'}, not submitted"
    review = payload.get("review") if isinstance(payload.get("review"), dict) else {}
    pr = payload.get("pull_request") if isinstance(payload.get("pull_request"), dict) else {}
    head = (pr.get("head") or {}).get("sha") or ""
    reason = approval_refusal(repo, review, head, own_logins())
    if reason:
        return None, reason
    found = next((s for s in combined_statuses(repo, head)
                  if isinstance(s, dict) and s.get("context") == HUMAN_CHECK_NAME), None)
    if found is None or found.get("state") != "pending":
        current = found.get("state") if found else "none"
        return None, f"the head's {HUMAN_CHECK_NAME} status is {current}, not pending"
    login = review["user"]["login"]
    run_url = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    return {"context": HUMAN_CHECK_NAME, "target_url": run_url, "state": "success",
            "description": f"Cleared by @{login}'s approval"[:140]}, None


def human_review_skip_status():
    """The "Tribunal human review" status for the event's head, which Tribunal did not review.

    None when the head already has one: an event that did not change the head (an edit, a reopen)
    must not replace the status a review or an approval left on it. `carry` looks, in order, at the
    previous head of a push (still readable after a force-push), the commit the action's latest
    review read, and the pull request's earlier commits, newest first.
    """
    mode = setting("human-review-on-skip", "carry")
    if mode not in HUMAN_SKIP_MODES:
        raise ValueError(f"human-review-on-skip must be one of {', '.join(repr(m) for m in HUMAN_SKIP_MODES)}")
    repo, payload = os.environ["GITHUB_REPOSITORY"], read(os.environ["GITHUB_EVENT_PATH"])
    pr = payload["pull_request"]
    head = pr["head"]["sha"]

    def statuses(sha):
        return combined_statuses(repo, sha)

    if any(isinstance(s, dict) and s.get("context") == HUMAN_CHECK_NAME for s in statuses(head)):
        return None
    if mode == "success":
        state = {"state": "success", "description": NOT_REVIEWED}
    elif mode == "pending":
        state = {"state": "pending", "description": f"{NOT_REVIEWED}; a person should review"}
    else:
        def candidates():
            before = payload.get("before")
            if payload.get("action") == "synchronize" and isinstance(before, str) and before and before != head:
                yield before, statuses(before)
            reviewed = latest_own_review_commit(repo, pr["number"])
            if reviewed:
                yield reviewed, statuses(reviewed), True
            shas = call("gh", "api", "--paginate", f"repos/{repo}/pulls/{pr['number']}/commits", "--jq", ".[].sha").split()
            for sha in reversed(shas):
                if sha != head:
                    yield sha, statuses(sha)
        state = carried_human_review(candidates())
    run_url = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    return {"context": HUMAN_CHECK_NAME, "target_url": run_url, **state}


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
    elif command == "findings-check":
        status, table, blocking, should_fix = findings_check(job)
        write(job / "status.json", status)
        (job / "findings-table.md").write_text(table)
        with open(os.environ.get("GITHUB_OUTPUT", os.devnull), "a") as outputs:
            outputs.write(f"blocking={'' if blocking is None else blocking}\nshould-fix={'' if should_fix is None else should_fix}\n")
        print(f"{status['state']}: {status['description']}")
    elif command == "human-review-check":
        status = human_review_status(job)
        write(job / "human-status.json", status)
        print(f"{status['state']}: {status['description']}")
    elif command == "human-review-skip":
        (job / "human-status.json").unlink(missing_ok=True)
        status = human_review_skip_status()
        if status is None:
            print(f"left as it is: the head already has a {HUMAN_CHECK_NAME} status")
        else:
            write(job / "human-status.json", status)
            print(f"{status['state']}: {status['description']}")
    elif command == "human-review-approve":
        (job / "human-status.json").unlink(missing_ok=True)
        status, reason = human_review_approval_status()
        if status is None:
            print(f"not cleared: {reason}")
        else:
            write(job / "human-status.json", status)
            print(f"{status['state']}: {status['description']}")
    elif command == "needs-human":
        flag = None
        (job / "needs-human.md").unlink(missing_ok=True)
        try:
            flag = needs_human(job)
            if flag:
                (job / "needs-human.md").write_text("\n".join(needs_human_reasons(job)) + "\n")
        except (OSError, ValueError) as error:
            flag = None
            print(f"needs-human: {error}", file=sys.stderr)
        with open(os.environ.get("GITHUB_OUTPUT", os.devnull), "a") as outputs:
            outputs.write(f"needs-human={'' if flag is None else str(flag).lower()}\n")
    elif command == "post-state":
        print(post_state(job) or "")
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
