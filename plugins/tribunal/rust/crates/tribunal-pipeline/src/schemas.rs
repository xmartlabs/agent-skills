//! The JSON Schemas model answers must satisfy, published under
//! `rust/schemas/` and checked against the workflow's constants by
//! `tests/diff_schemas.rs`.

use serde_json::Value;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("a published schema is valid JSON")
}

/// `FINDINGS_SCHEMA`: what a lens returns.
#[must_use]
pub fn findings() -> Value {
    parse(include_str!("../../../schemas/findings.schema.json"))
}

/// What a `--scenario-questions` generator returns: the questions it wants
/// answered, each anchored at a file and, when it can, a line.
#[must_use]
pub fn scenario_questions() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "file": {"type": "string"},
                        "line": {"type": "number"},
                        "question": {"type": "string"}
                    },
                    "required": ["file", "question"]
                }
            }
        },
        "required": ["questions"]
    })
}

/// [`findings`] with the optional `runtimeClaim` a lens fills under
/// `--runtime-confirm`.
#[must_use]
pub fn findings_with_runtime_claim() -> Value {
    let mut s = findings();
    if let Some(props) = s
        .pointer_mut("/properties/findings/items/properties")
        .and_then(Value::as_object_mut)
    {
        props.insert(
            "runtimeClaim".into(),
            serde_json::json!({
                "type": ["object", "null"],
                "properties": {
                    "function": {"type": "string"},
                    "args": {"type": "array"},
                    "expect": {
                        "type": "object",
                        "properties": {
                            "returns": {},
                            "throws": {"type": "boolean"}
                        }
                    }
                },
                "required": ["function", "args", "expect"]
            }),
        );
    }
    s
}

/// `BOT_VETTING_SCHEMA`: what the bot-comment vetting pass returns.
#[must_use]
pub fn bot_vetting() -> Value {
    parse(include_str!("../../../schemas/bot-vetting.schema.json"))
}

/// The thread harvest's answer: `PR_CONTEXT_SCHEMA`'s `inThreadDecisions`
/// and `FETCH_PR_SCHEMA`'s `priorReviewSummaries`, without the relay fields
/// a native fetch has no use for.
#[must_use]
pub fn thread_harvest() -> Value {
    parse(include_str!("../../../schemas/thread-harvest.schema.json"))
}

/// `DEDUP_SCHEMA`: the pre-panel dedup's clusters.
#[must_use]
pub fn dedup() -> Value {
    parse(include_str!("../../../schemas/dedup.schema.json"))
}

/// The `verify-prior` dispatch's answer.
#[must_use]
pub fn prior_verification() -> Value {
    parse(include_str!(
        "../../../schemas/prior-verification.schema.json"
    ))
}

/// `REFUTATION_SCHEMA`: one refuter's vote.
#[must_use]
pub fn refutation() -> Value {
    parse(include_str!("../../../schemas/refutation.schema.json"))
}

/// `BATCH_REFUTATION_SCHEMA`: a batched refuter's votes.
#[must_use]
pub fn batch_refutation() -> Value {
    parse(include_str!(
        "../../../schemas/batch-refutation.schema.json"
    ))
}

/// `STRENGTHEN_SCHEMA`: the refine pass's answer.
#[must_use]
pub fn strengthen() -> Value {
    parse(include_str!("../../../schemas/strengthen.schema.json"))
}

/// `ADJUDICATOR_SCHEMA`: the debate judge's ruling.
#[must_use]
pub fn adjudicator() -> Value {
    parse(include_str!("../../../schemas/adjudicator.schema.json"))
}

/// `CRITIC_SCHEMA`: the completeness critic's answer.
#[must_use]
pub fn critic() -> Value {
    parse(include_str!("../../../schemas/critic.schema.json"))
}

/// `PANEL_TIEBREAK_SCHEMA`: the panel-ceiling tie-break's order.
#[must_use]
pub fn panel_tiebreak() -> Value {
    parse(include_str!("../../../schemas/panel-tiebreak.schema.json"))
}
