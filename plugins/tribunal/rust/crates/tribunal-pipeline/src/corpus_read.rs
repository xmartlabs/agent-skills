//! `tribunal corpus read`: one agent per candidate item reads the code at the
//! entry's pin and the fix, and proposes a verdict and a label. The proposal
//! is written into the item; a person decides, so `status` and `tier` never
//! change here.
//!
//! Workers take whole entries, so at most `concurrency` worktrees exist at
//! once, each a detached checkout of one pin shared by that entry's items.
//! Before each dispatch the spend so far, the dispatches running and the
//! next one are priced at the mean cost seen (or [`FIRST_ESTIMATE_USD`] before
//! any), and nothing starts past `max_usd`: a worker that does not fit waits
//! for the running dispatches, and the cap is reached only when none is
//! running. Worktrees are added and removed one at a time. A re-run skips items that already
//! carry a proposal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tribunal_agent::{Agent, DispatchRequest};
use tribunal_context::corpus::schema::violations;
use tribunal_context::exec::Tools;

use crate::panel::join_all;

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
pub const DEFAULT_EFFORT: &str = "medium";
pub const DEFAULT_CONCURRENCY: usize = 4;
/// The price of a dispatch before any has finished.
pub const FIRST_ESTIMATE_USD: f64 = 0.10;
const TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct ReadArgs {
    pub input: PathBuf,
    pub checkout: PathBuf,
    pub max_usd: f64,
    pub model: String,
    pub effort: String,
    pub concurrency: usize,
    /// Where the worktrees go.
    pub tmp: PathBuf,
    /// Written into each proposal, `YYYY-MM-DD`.
    pub date: String,
}

#[must_use]
pub fn answer_schema() -> Value {
    json!({
        "type": "object",
        "required": ["verdict", "description", "reasoning"],
        "properties": {
            "verdict": {"type": "string", "enum": ["defect", "minor", "not-a-defect", "cannot-tell"]},
            "severity": {"type": ["string", "null"], "enum": ["important", "nit", null]},
            "description": {"type": "string"},
            "quote": {"type": ["string", "null"]},
            "reasoning": {"type": "string"}
        }
    })
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

/// The question put to the reader. It names no language, framework or file
/// type: the agent reads what it needs.
#[must_use]
pub fn prompt(entry: &Value, item: &Value) -> String {
    let line = item["line"]
        .as_u64()
        .map(|n| format!(":{n}"))
        .unwrap_or_default();
    format!(
        "You are checking one point that an automated code reviewer raised on pull request #{pr} of {repo}, to decide whether it describes a real defect. The working directory is a checkout of commit {pin}, the pin.\n\n\
The reviewer commented on {file}{line} at commit {seen}. The author later changed the code in commit {fix}, and the reviewer marked its comment as addressed.\n\n\
The reviewer's comment:\n<comment>\n{text}\n</comment>\n\n\
Read the file and anything else you need at the pin. Run `git show {fix}` to see what the fix changed. Then decide:\n\
- defect: the code at the pin does the wrong thing for an input or state that a user or caller can reach, and the fix changes that;\n\
- minor: true, but only style, naming, hardening with no reachable failure, or a gap in tests or documentation;\n\
- not-a-defect: the claim is wrong at the pin, or the fix did something else;\n\
- cannot-tell: the code does not let you decide.\n\n\
For a defect or a minor point, give a severity: important when the failure reaches users or data, nit otherwise. Describe the problem in your own words, as someone who read the code would, without copying the reviewer's wording. Quote one whole line of {file} at the pin that the problem rests on, exactly as it appears there, trimmed. Say in reasoning what you read and what decided it.",
        pr = entry["pr"],
        repo = s(entry, "repo"),
        pin = s(entry, "reviewedSha"),
        file = s(item, "file"),
        seen = s(item, "seenAt"),
        fix = s(item, "fix"),
        text = s(item, "description"),
    )
}

#[derive(Debug, Default)]
struct Spend {
    spent: f64,
    done: usize,
    running: usize,
    stopped: bool,
}

impl Spend {
    fn estimate(&self) -> f64 {
        if self.done == 0 {
            FIRST_ESTIMATE_USD
        } else {
            self.spent / self.done as f64
        }
    }
}

/// What the workers share.
struct Shared {
    spend: Mutex<Spend>,
    tally: Mutex<Tally>,
    /// Held while a worktree is added or removed.
    worktrees: tokio::sync::Mutex<()>,
}

#[derive(Debug, Default)]
struct Tally {
    read: usize,
    failed: usize,
    verdicts: BTreeMap<String, usize>,
    errors: Vec<String>,
}

async fn git(tools: &Tools, dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = tools
        .git(args, Some(dir))
        .await
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.ok() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            out.stderr.trim()
        ))
    }
}

fn pending(entry: &Value) -> Vec<usize> {
    entry["knownReal"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .filter(|(_, i)| i.get("proposal").is_none())
                .map(|(n, _)| n)
                .collect()
        })
        .unwrap_or_default()
}

/// The candidate files under `dir`, sorted, with their entries.
fn load(dir: &Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("pr-") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    let mut out = Vec::new();
    for p in files {
        let text =
            std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", p.display()))?;
        if v["status"] == "candidate" {
            out.push((p, v));
        }
    }
    Ok(out)
}

fn write(path: &Path, entry: &Value) -> Result<(), String> {
    let bad = violations(entry);
    if !bad.is_empty() {
        return Err(format!(
            "{} would not match the corpus schema: {}",
            path.display(),
            bad.join("; ")
        ));
    }
    let mut text = serde_json::to_string_pretty(entry).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Reads one entry's pending items in a worktree of its pin, writing the file
/// after each answer. The worktree is removed whatever happens.
async fn read_entry<A: Agent>(
    agent: &A,
    tools: &Tools,
    args: &ReadArgs,
    path: &Path,
    mut entry: Value,
    sh: &Shared,
) {
    let pin = s(&entry, "reviewedSha").to_owned();
    let name = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("entry")
        .to_owned();
    let wt = args.tmp.join(format!(
        "tribunal-corpus-read-{}-{name}",
        std::process::id()
    ));
    let wt_s = wt.display().to_string();
    // Concurrent `git worktree add` on one repository can read each other's
    // half-written entries under .git/worktrees.
    let added = {
        let _one_at_a_time = sh.worktrees.lock().await;
        git(
            tools,
            &args.checkout,
            &["worktree", "add", "--quiet", "--detach", &wt_s, &pin],
        )
        .await
    };
    if let Err(e) = added {
        sh.tally.lock().unwrap().errors.push(format!("{name}: {e}"));
        return;
    }
    for idx in pending(&entry) {
        // Over the cap with dispatches still running, wait for them: their
        // real cost may leave room. With none running, the cap is reached.
        let admitted = loop {
            {
                let mut sp = sh.spend.lock().unwrap();
                if sp.stopped {
                    break false;
                }
                let est = sp.estimate();
                if sp.spent + (sp.running as f64 + 1.0) * est <= args.max_usd {
                    sp.running += 1;
                    break true;
                }
                if sp.running == 0 {
                    sp.stopped = true;
                    break false;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        if !admitted {
            break;
        }
        let item = entry["knownReal"][idx].clone();
        let mut req = DispatchRequest::new(
            format!("corpus-read:{}:{}", entry["pr"], s(&item, "id")),
            prompt(&entry, &item),
            &wt,
        );
        req.model = Some(args.model.clone());
        req.effort = Some(args.effort.clone());
        req.tools = vec!["Read".to_owned(), "Bash".to_owned()];
        req.schema = Some(answer_schema());
        req.timeout = TIMEOUT;
        let res = agent.run(&req).await;
        let cost = res.output.as_ref().map_or(0.0, |o| o.total_cost_usd);
        {
            let mut sp = sh.spend.lock().unwrap();
            sp.running -= 1;
            sp.spent += cost;
            sp.done += 1;
        }
        let Some(ans) = res.structured().cloned() else {
            let mut t = sh.tally.lock().unwrap();
            t.failed += 1;
            t.errors.push(format!(
                "{name} {}: the dispatch returned no answer ({:?})",
                s(&item, "id"),
                res.exit
            ));
            continue;
        };
        let quote = ans["quote"]
            .as_str()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let quote_found = match quote {
            None => Value::Null,
            Some(q) => {
                let file = git(
                    tools,
                    &args.checkout,
                    &["show", &format!("{pin}:{}", s(&item, "file"))],
                )
                .await;
                json!(file.is_ok_and(|text| text.lines().any(|l| l.trim() == q)))
            }
        };
        let verdict = s(&ans, "verdict").to_owned();
        entry["knownReal"][idx]["proposal"] = json!({
            "verdict": verdict,
            "severity": ans.get("severity").cloned().unwrap_or(Value::Null),
            "description": s(&ans, "description"),
            "quote": quote,
            "quoteFound": quote_found,
            "reasoning": ans.get("reasoning").cloned().unwrap_or(Value::Null),
            "model": args.model,
            "costUsd": (cost * 10_000.0).round() / 10_000.0,
            "date": args.date,
        });
        {
            let mut t = sh.tally.lock().unwrap();
            t.read += 1;
            *t.verdicts.entry(verdict).or_default() += 1;
        }
        if let Err(e) = write(path, &entry) {
            sh.tally.lock().unwrap().errors.push(e);
        }
    }
    let removed = {
        let _one_at_a_time = sh.worktrees.lock().await;
        git(
            tools,
            &args.checkout,
            &["worktree", "remove", "--force", &wt_s],
        )
        .await
    };
    if let Err(e) = removed {
        sh.tally.lock().unwrap().errors.push(format!("{name}: {e}"));
    }
}

/// Reads every pending item it can within the cap. Returns the summary.
///
/// # Errors
/// An input directory that cannot be read, or a cap that is not positive.
pub async fn read<A: Agent>(agent: &A, tools: &Tools, args: &ReadArgs) -> Result<Value, String> {
    if args.max_usd.is_nan() || args.max_usd <= 0.0 {
        return Err("--max-usd must be a positive number of dollars".to_owned());
    }
    let entries: Vec<(PathBuf, Value)> = load(&args.input)?
        .into_iter()
        .filter(|(_, e)| !pending(e).is_empty())
        .collect();
    let total: usize = entries.iter().map(|(_, e)| pending(e).len()).sum();
    let queue = Mutex::new(entries);
    let sh = Shared {
        spend: Mutex::new(Spend::default()),
        tally: Mutex::new(Tally::default()),
        worktrees: tokio::sync::Mutex::new(()),
    };
    join_all((0..args.concurrency.max(1)).map(|_| async {
        loop {
            if sh.spend.lock().unwrap().stopped {
                break;
            }
            let next = queue.lock().unwrap().pop();
            let Some((path, entry)) = next else { break };
            read_entry(agent, tools, args, &path, entry, &sh).await;
        }
    }))
    .await;
    let sp = sh.spend.into_inner().unwrap();
    let t = sh.tally.into_inner().unwrap();
    Ok(json!({
        "itemsPending": total,
        "itemsRead": t.read,
        "failed": t.failed,
        "verdicts": t.verdicts,
        "costUsd": (sp.spent * 100.0).round() / 100.0,
        "itemsLeft": total - t.read,
        "stoppedAtCap": sp.stopped,
        "maxUsd": args.max_usd,
        "errors": t.errors,
    }))
}

#[must_use]
pub fn summary_text(s: &Value) -> String {
    let verdicts = s["verdicts"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "none".to_owned());
    let mut out = format!(
        "{} of {} pending item(s) read ({verdicts}), {} failed, US${} spent.\n",
        s["itemsRead"], s["itemsPending"], s["failed"], s["costUsd"]
    );
    if s["stoppedAtCap"] == true {
        out.push_str(&format!(
            "Stopped at the US${} cap with {} item(s) left. Run again with a higher --max-usd to read them.\n",
            s["maxUsd"], s["itemsLeft"]
        ));
    }
    for e in s["errors"].as_array().into_iter().flatten() {
        out.push_str(&format!("error: {}\n", e.as_str().unwrap_or("")));
    }
    out.push_str("Proposals are leads: a person accepts or rejects each one.\n");
    out
}
