//! The result object `claude -p --output-format json` prints on stdout.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// The CLI's own estimate at list price, not a bill.
    #[serde(default, rename = "costUSD")]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CliResult {
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    /// The final text. With a schema it is the answer serialized as JSON.
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub num_turns: u32,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub total_cost_usd: f64,
    #[serde(default, rename = "modelUsage")]
    pub model_usage: BTreeMap<String, ModelUsage>,
    #[serde(default)]
    pub permission_denials: Vec<Value>,
}

impl CliResult {
    /// Reads the last JSON object on stdout. The CLI prints one; anything
    /// before it (a warning a hook echoed, say) is not the result.
    pub fn parse(stdout: &str) -> Result<Self, String> {
        let line = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .ok_or_else(|| "no JSON result on stdout".to_owned())?;
        serde_json::from_str(line).map_err(|e| format!("unreadable result: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_fields_a_review_uses() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"word\":\"hello\"}","structured_output":{"word":"hello"},"num_turns":4,"session_id":"s","duration_ms":10,"total_cost_usd":0.01,"modelUsage":{"claude-haiku-5-5":{"inputTokens":4,"outputTokens":990,"cacheReadInputTokens":11089,"cacheCreationInputTokens":13896,"costUSD":0.0034}}}"#;
        let r = CliResult::parse(&format!("noise\n{out}\n")).unwrap();
        assert_eq!(
            r.structured_output,
            Some(serde_json::json!({"word": "hello"}))
        );
        assert_eq!(r.num_turns, 4);
        assert_eq!(r.model_usage["claude-haiku-5-5"].output_tokens, 990);
    }

    #[test]
    fn no_result_is_an_error() {
        assert!(CliResult::parse("plain text\n").is_err());
    }
}
