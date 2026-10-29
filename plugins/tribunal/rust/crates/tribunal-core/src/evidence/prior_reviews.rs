//! Port of the PRIOR-REVIEWS region: which human reviews still request
//! changes, the viewer's own standing review, and the post gate built on both.
//!
//! Reviews are the harvester's verbatim copy of GitHub's, so they are read as
//! JSON: every field goes through `String(x || "")` as in JavaScript.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde_json::{json, Value};

use super::js_ext;
use crate::js;

const BOT_NAMES: [&str; 4] = ["coderabbitai", "github-actions", "dependabot", "renovate"];

/// `BOT_LOGIN`: `/(\[bot\]$)|^(coderabbitai|...)$/i`. Without the `u` flag only
/// ASCII letters fold, so `ſ` never matches `s`.
pub fn is_bot_login(login: &str) -> bool {
    let b = login.as_bytes();
    let tagged = b.len() >= 5 && b[b.len() - 5..].eq_ignore_ascii_case(b"[bot]");
    tagged || BOT_NAMES.iter().any(|n| login.eq_ignore_ascii_case(n))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub author: String,
    pub state: String,
    pub at: Option<String>,
    pub summary: String,
}

impl Review {
    pub fn to_value(&self) -> Value {
        json!({
            "author": self.author,
            "state": self.state,
            "at": self.at,
            "summary": self.summary,
        })
    }
}

fn field(r: &Value, key: &str) -> String {
    js_ext::string_or_empty(js::get(r, key))
}

fn trimmed(r: &Value, key: &str) -> String {
    js::trim(&field(r, key)).to_owned()
}

fn upper_state(r: &Value) -> String {
    field(r, "state").to_uppercase()
}

/// Ports `openChangeRequests`. An entry with no timestamp never supersedes
/// one that has one, and timestamps compare as JavaScript strings do.
pub fn open_change_requests(raw: &Value) -> Vec<Review> {
    let list: &[Value] = match raw {
        Value::Array(a) => a,
        _ => &[],
    };
    let humans: Vec<&Value> = list
        .iter()
        .filter(|r| {
            let author = trimmed(r, "author");
            js::truthy(Some(r)) && !author.is_empty() && !is_bot_login(&author)
        })
        .collect();
    let mut latest_other: HashMap<String, String> = HashMap::new();
    for r in &humans {
        if upper_state(r) == "CHANGES_REQUESTED" {
            continue;
        }
        let key = trimmed(r, "author").to_lowercase();
        let at = trimmed(r, "at");
        if at.is_empty() {
            continue;
        }
        let later = latest_other
            .get(&key)
            .is_none_or(|cur| js_ext::utf16_cmp(&at, cur) == Ordering::Greater);
        if later {
            latest_other.insert(key, at);
        }
    }
    humans
        .into_iter()
        .filter(|r| upper_state(r) == "CHANGES_REQUESTED")
        .filter(|r| {
            let superseding = latest_other.get(&trimmed(r, "author").to_lowercase());
            let at = trimmed(r, "at");
            !superseding.is_some_and(|s| {
                !s.is_empty() && !at.is_empty() && js_ext::utf16_cmp(s, &at) == Ordering::Greater
            })
        })
        .map(|r| {
            let at = trimmed(r, "at");
            Review {
                author: trimmed(r, "author"),
                state: "CHANGES_REQUESTED".into(),
                at: (!at.is_empty()).then_some(at),
                summary: trimmed(r, "summary"),
            }
        })
        .collect()
}

/// Ports `ownStandingReview(allReviews, viewer)`: the viewer's latest review
/// of any state but DISMISSED, ignoring undated ones. On equal timestamps the
/// earlier entry wins, as the strict `>` in the JavaScript reduce leaves it.
pub fn own_standing_review(all_reviews: &Value, viewer: Option<&Value>) -> Option<Review> {
    let me = js::trim(&js_ext::string_or_empty(viewer)).to_lowercase();
    if me.is_empty() {
        return None;
    }
    let list: &[Value] = match all_reviews {
        Value::Array(a) => a,
        _ => &[],
    };
    let latest = list
        .iter()
        .filter(|r| {
            js::truthy(Some(r))
                && trimmed(r, "author").to_lowercase() == me
                && upper_state(r) != "DISMISSED"
                && !trimmed(r, "at").is_empty()
        })
        .reduce(|a, b| {
            if js_ext::utf16_cmp(&trimmed(b, "at"), &trimmed(a, "at")) == Ordering::Greater {
                b
            } else {
                a
            }
        })?;
    Some(Review {
        author: trimmed(latest, "author"),
        state: upper_state(latest),
        at: Some(trimmed(latest, "at")),
        summary: trimmed(latest, "summary"),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostGate {
    /// `clear`, `own-open-review`, `own-prior-review` or `unknown-viewer`.
    pub status: &'static str,
    pub own_open_review: Option<Review>,
    pub own_standing_review: Option<Review>,
}

impl PostGate {
    fn bare(status: &'static str) -> Self {
        PostGate {
            status,
            own_open_review: None,
            own_standing_review: None,
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "status": self.status,
            "ownOpenReview": self.own_open_review.as_ref().map(Review::to_value),
            "ownStandingReview": self.own_standing_review.as_ref().map(Review::to_value),
        })
    }
}

/// Ports `postGate(priorReviews, viewer, allReviews)`. `prior_reviews` is
/// `openChangeRequests`' output; `all_reviews` is the raw list, `None` when
/// the caller passed nothing.
pub fn post_gate(
    prior_reviews: Option<&[Review]>,
    viewer: Option<&Value>,
    all_reviews: Option<&Value>,
) -> PostGate {
    let open = prior_reviews.unwrap_or(&[]);
    let all: &[Value] = match all_reviews {
        Some(Value::Array(a)) => a,
        _ => &[],
    };
    let me = js::trim(&js_ext::string_or_empty(viewer)).to_lowercase();
    if !open.is_empty() {
        if me.is_empty() {
            return PostGate::bare("unknown-viewer");
        }
        if let Some(mine) = open.iter().find(|r| r.author.to_lowercase() == me) {
            return PostGate {
                status: "own-open-review",
                own_open_review: Some(mine.clone()),
                own_standing_review: None,
            };
        }
    }
    if all.is_empty() {
        return PostGate::bare("clear");
    }
    if me.is_empty() {
        return PostGate::bare("unknown-viewer");
    }
    // The JavaScript passes the already-lowered login back in, and
    // `ownStandingReview` trims and lowers it again.
    let all_value = Value::Array(all.to_vec());
    match own_standing_review(&all_value, Some(&Value::String(me))) {
        None => PostGate::bare("clear"),
        Some(standing) => PostGate {
            status: "own-prior-review",
            own_open_review: None,
            own_standing_review: Some(standing),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rv(author: &str, state: &str, at: &str) -> Value {
        json!({"author": author, "state": state, "at": at, "summary": ""})
    }

    #[test]
    fn bots_and_superseded_requests_are_not_open() {
        let raw = json!([
            rv("alice", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z"),
            rv("bob", "CHANGES_REQUESTED", "2026-07-21T10:00:00Z"),
            rv("BOB", "APPROVED", "2026-07-22T10:00:00Z"),
            rv("CodeRabbitAI", "CHANGES_REQUESTED", "2026-07-21T11:00:00Z"),
            rv("x[BOT]", "CHANGES_REQUESTED", "2026-07-21T11:00:00Z"),
        ]);
        let open = open_change_requests(&raw);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].author, "alice");
    }

    #[test]
    fn a_standing_approval_of_mine_gates_posting() {
        let all = json!([rv("Alice", "APPROVED", "2026-07-21T10:00:00Z")]);
        let g = post_gate(Some(&[]), Some(&json!("alice")), Some(&all));
        assert_eq!(g.status, "own-prior-review");
        assert_eq!(
            post_gate(Some(&[]), Some(&json!("")), Some(&all)).status,
            "unknown-viewer"
        );
        assert_eq!(
            post_gate(Some(&[]), Some(&json!("")), Some(&json!([]))).status,
            "clear"
        );
    }

    #[test]
    fn my_open_block_outranks_a_standing_review() {
        let open = open_change_requests(&json!([rv("Alice", "CHANGES_REQUESTED", "t")]));
        assert_eq!(
            post_gate(Some(&open), Some(&json!("alice")), None).status,
            "own-open-review"
        );
        assert_eq!(post_gate(Some(&open), None, None).status, "unknown-viewer");
    }
}
