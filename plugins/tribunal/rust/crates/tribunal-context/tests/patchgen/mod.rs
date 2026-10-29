//! Generated unified diffs for the section-diff and evidence-resolver
//! differentials, and real ones taken from this repository's own history.
//!
//! The generator mixes what `git diff` writes (renames, binary files, new and
//! deleted files, `\ No newline at end of file`, several hunks) with what it
//! does not but a scraped or truncated patch can hold: quoted and malformed
//! headers, CRLF and U+2028 inside headers, astral characters, hunk starts
//! past 2^53, banners, and lines long enough to cross the 6,000-unit excerpt
//! cut inside a surrogate pair.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use proptest::prelude::*;
use proptest::sample::select;

pub const PATHS: &[&str] = &[
    "src/a.ts",
    "src/b.ts",
    "lib/x y.ts",
    "src/é/ü.ts",
    "src/😀.ts",
    "a b/c b/d.ts",
    // The b-side of the header above under a greedy split.
    "d.ts",
    "src/undefined",
    "src/null",
    "x/5",
    "README",
    "src/a.ts.bak",
    "true",
];

pub const CONTENTS: &[&str] = &[
    "const total = computeTotal(items);",
    "  const total = computeTotal(items);  ",
    "const   total =\tcomputeTotal(items);",
    "return null;",
    "}",
    "{",
    "} else {",
    "return;",
    "short",
    "if (user.isAdmin) {",
    "émoji 😀 line with astral 𝒳 chars",
    "a\u{FEFF}b spaced\u{3000}out more",
    "trailing carriage return here\r",
    "has\u{2028}separator inside it",
    "No relevant quotes found",
    "await prisma.invoice.findMany({ where });",
    "",
    "   ",
    "\u{0085}nel is not whitespace in js",
    "+ starts with a plus already",
];

const RAW_LINES: &[&str] = &[
    "\\ No newline at end of file",
    "+++ b/odd.ts",
    "--- a/odd.ts",
    "@@ -x +1 @@",
    "@@ -3 +3@@",
    "@@ -1,2 +1 @@@",
    "garbage between hunks",
    "diff --git a/src/a.ts b/src/a.ts",
    "\t tab-led line",
    "Binary files a/img.png and b/img.png differ",
];

fn starts() -> Vec<String> {
    let mut v: Vec<String> = [
        "0",
        "1",
        "2",
        "3",
        "5",
        "7",
        "10",
        "12",
        "20",
        "40",
        "98",
        "007",
        "9007199254740993",
        "123456789012345678901234567890",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    v.push(format!("1{}", "0".repeat(400)));
    v
}

fn body_line() -> BoxedStrategy<String> {
    prop_oneof![
        40 => (select(&["+", "-", " "][..]), select(CONTENTS))
            .prop_map(|(p, c)| format!("{p}{c}")),
        6 => Just(String::new()),
        6 => select(RAW_LINES).prop_map(str::to_owned),
        // Long enough to cross the excerpt cut, at either parity.
        1 => (select(&["+", "-", " "][..]), 0usize..3000, 0usize..2000)
            .prop_map(|(p, a, m)| format!("{p}{}{}", "a".repeat(a), "😀".repeat(m))),
    ]
    .boxed()
}

fn hunk() -> BoxedStrategy<Vec<String>> {
    let count = proptest::option::of(select(&["0", "1", "4", "30"][..]));
    (
        select(starts()),
        count.clone(),
        select(starts()),
        count,
        select(&["", " function first() {", "@"][..]),
        proptest::collection::vec(body_line(), 1..14),
    )
        .prop_map(|(o, oc, n, nc, tail, body)| {
            let c = |x: Option<&str>| x.map(|x| format!(",{x}")).unwrap_or_default();
            let mut lines = vec![format!("@@ -{o}{} +{n}{} @@{tail}", c(oc), c(nc))];
            lines.extend(body);
            lines
        })
        .boxed()
}

/// One file's part of the diff: its lines and the post-image path the
/// generator meant (not always what a parser recovers).
fn file_part() -> BoxedStrategy<(Vec<String>, String)> {
    (
        0usize..12,
        select(PATHS),
        select(PATHS),
        proptest::collection::vec(hunk(), 0..4),
    )
        .prop_map(|(kind, p, old, hunks)| {
            let mut lines: Vec<String> = match kind {
                1 => vec![
                    format!("diff --git a/{old} b/{p}"),
                    "similarity index 90%".into(),
                    format!("rename from {old}"),
                    format!("rename to {p}"),
                ],
                2 => vec![format!("diff --git \"a/{p}\\t\" \"b/{p}\\t\"")],
                3 => vec![
                    format!("diff --git a/{p} b/{p}"),
                    "new file mode 100644".into(),
                    "--- /dev/null".into(),
                    format!("+++ b/{p}"),
                ],
                4 => vec![
                    format!("diff --git a/{p} b/{p}"),
                    "deleted file mode 100644".into(),
                    format!("--- a/{p}"),
                    "+++ /dev/null".into(),
                ],
                5 => vec![
                    format!("diff --git a/{p} b/{p}"),
                    "index 1111111..2222222".into(),
                    format!("Binary files a/{p} and b/{p} differ"),
                ],
                6 => vec![format!("diff --git a/{p} b/{p}\r")],
                7 => vec![format!("diff --git a/{p}\u{2028} b/{p}")],
                8 => vec![format!("diff --git a/{p}")],
                9 => vec![format!("diff --git a/{p} b/")],
                _ => vec![
                    format!("diff --git a/{p} b/{p}"),
                    "index 1111111..2222222 100644".into(),
                    format!("--- a/{p}"),
                    format!("+++ b/{p}"),
                ],
            };
            for h in hunks {
                lines.extend(h);
            }
            (lines, p.to_owned())
        })
        .boxed()
}

/// A diff and the paths its headers meant.
pub fn diff() -> BoxedStrategy<(String, Vec<String>)> {
    (
        prop_oneof![
            16 => Just(""),
            2 => Just("From 1234 Mon Sep 17 00:00:00 2001\nSubject: widgets"),
            1 => Just("Output too large (81KB). Full output saved to: /x"),
            1 => Just("  \r\n Full diff available at: https://example.com/acme/widgets-api"),
            1 => Just("Preview (first 2KB):"),
        ],
        proptest::collection::vec(file_part(), 0..6),
        any::<bool>(),
    )
        .prop_map(|(pre, parts, trailing)| {
            let mut lines: Vec<String> = Vec::new();
            if !pre.is_empty() {
                lines.push(pre.to_owned());
            }
            let mut paths = Vec::new();
            for (l, p) in parts {
                lines.extend(l);
                paths.push(p);
            }
            let mut text = lines.join("\n");
            if trailing {
                text.push('\n');
            }
            (text, paths)
        })
        .boxed()
}

/// `git show` of the last `n` commits of this repository, renames detected:
/// real patches, whatever the history holds when the test runs.
pub fn repo_patches(n: usize) -> Vec<(String, String)> {
    repo_patches_in(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.."),
        n,
    )
}

/// [`repo_patches`] for the repository at `root`. Git runs without the
/// user's or the system's config, which can drop the `a/` and `b/` prefixes
/// the callers read paths from, and a shallow history shorter than `n` fails
/// here naming what it found rather than as a thin sample further on: a
/// runner's checkout left shallow gave 1 commit of 25. A complete history
/// shorter than `n`, as in a young repository, is used whole.
pub fn repo_patches_in(root: &Path, n: usize) -> Vec<(String, String)> {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git must be on PATH");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let shas: Vec<String> = git(&[
        "rev-list",
        "--no-merges",
        &format!("--max-count={n}"),
        "HEAD",
    ])
    .lines()
    .map(str::to_owned)
    .collect();
    let shallow = git(&["rev-parse", "--is-shallow-repository"]).trim() == "true";
    assert!(
        shas.len() == n || (!shallow && !shas.is_empty()),
        "{} has {} commits that are not merges from HEAD, {n} wanted; shallow: {shallow}",
        root.display(),
        shas.len(),
    );
    shas.into_iter()
        .map(|sha| {
            let show = git(&[
                "show",
                "-M",
                "--format=",
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                &sha,
            ]);
            (sha, show)
        })
        .collect()
}
