#!/usr/bin/env python3
"""Pure renderer for release notes: intermediate JSON (stdin) -> grouped markdown (stdout).

No network, no VCS — a deterministic function of its input, so it can be unit-tested with
fixtures. The orchestrator (release-notes.sh) is responsible for all git/gh I/O and for building
the JSON document this reads. Schema: see the fixtures in tests/release_notes_test.sh.
"""
import json
import re
import sys
from urllib.parse import urlparse

# Group order and headings. Only non-empty groups are emitted, in this order.
GROUPS = [
    ("feature", "### 🚀 Features"),
    ("fix", "### 🐛 Fixes"),
    ("chore", "### 🧹 Chores"),
    ("other", "### 📦 Other"),
]

# Only these hosts render inline in a GitHub comment/release on a private repo (see parent design
# §1). A raw private-repo URL must never be embedded as an image — it would show broken.


def _clean_title(title, ticket_key):
    """Drop a leading '[KEY] - ' / '[KEY] ' prefix (the key is shown separately)."""
    t = title.strip()
    t = re.sub(r"^\[[^\]]+\]\s*[-–:]?\s*", "", t)
    return t.strip() or title.strip()


def _embeddable(url):
    # Match the parsed host exactly: a substring check lets https://evil.example/?githubusercontent.com/ through.
    try:
        p = urlparse(url)
    except ValueError:
        return False
    host = (p.hostname or "").rstrip(".").lower()
    if p.scheme != "https":
        return False
    return ((host == "github.com" and p.path.startswith("/user-attachments/assets/"))
            or host.endswith(".githubusercontent.com"))


def _pr_line(pr):
    num = pr.get("number")
    url = pr.get("url") or ""
    pr_md = f"[#{num}]({url})" if url else f"#{num}"

    ticket = pr.get("ticket") or {}
    key = ticket.get("key")
    turl = ticket.get("url")
    if key and turl:
        lead = f"**[{key}]({turl})** "
    elif key:
        lead = f"**{key}** "
    else:
        lead = ""

    title = _clean_title(pr.get("title", ""), key)
    line = f"- {lead}{title} ({pr_md})"

    ev = pr.get("evidence") or None
    lines = [line]
    if ev:
        comment_url = ev.get("commentUrl") or url
        images = [u for u in (ev.get("images") or []) if _embeddable(u)]
        links = [u for u in (ev.get("links") or []) if _embeddable(u)]
        suffix = f" — [evidence]({comment_url})" if comment_url else ""
        for i, lk in enumerate(links):
            suffix += f" · [video{('' if len(links) == 1 else ' ' + str(i + 1))}]({lk})"
        lines[0] = line + suffix
        if images:
            # First image inline (indented to stay in the list item); notes stay skimmable.
            lines.append(f"  ![]({images[0]})")
    return "\n".join(lines)


def render(doc):
    prs = doc.get("prs") or []
    rng = doc.get("range") or ""
    title = doc.get("title") or rng or "Release notes"

    with_ev = sum(1 for p in prs if p.get("evidence"))
    out = [f"## {title}", ""]
    out.append(f"_{len(prs)} change{'' if len(prs) == 1 else 's'} · {with_ev} with evidence_")
    out.append("")

    by_type = {}
    for p in prs:
        t = (p.get("type") or "other").lower()
        if t not in {g[0] for g in GROUPS}:
            t = "other"
        by_type.setdefault(t, []).append(p)

    any_group = False
    for key, heading in GROUPS:
        group = by_type.get(key)
        if not group:
            continue
        any_group = True
        out.append(heading)
        out.append("")
        for p in group:
            out.append(_pr_line(p))
        out.append("")

    if not any_group:
        out.append("_No merged pull requests in this range._")
        out.append("")

    return "\n".join(out).rstrip() + "\n"


def main():
    raw = sys.stdin.read()
    try:
        doc = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError as e:
        sys.stderr.write(f"render-notes: invalid JSON on stdin: {e}\n")
        return 1
    sys.stdout.write(render(doc))
    return 0


if __name__ == "__main__":
    sys.exit(main())
