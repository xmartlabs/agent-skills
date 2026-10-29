//! Port of the CONVENTIONS-CHANNEL region: the repo's own convention file as
//! SKILL.md loaded it, the manifest the metrics row records, and the line
//! each reviewer and refuter prompt carries.
//!
//! The region's `checkBash` (a `node -e` sha256 check spliced into bash) is not
//! ported: the binary reads and hashes the file itself.

use serde_json::{Map, Value};

use super::js_ext;
use crate::js;

/// What `conventionsChannel` returns, minus `checkBash`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConventionsChannel {
    /// The metrics row's record of the loaded file; `None` when there is none.
    pub manifest: Option<Value>,
    line: String,
}

impl ConventionsChannel {
    /// `withConventions`: the conventions line, a blank line, the prompt.
    pub fn with_conventions(&self, prompt: &str) -> String {
        if self.line.is_empty() {
            prompt.to_owned()
        } else {
            format!("{}\n\n{prompt}", self.line)
        }
    }

    /// `reviewerPrompt`: the methodology line, the conventions line under it,
    /// a blank line, the prompt.
    pub fn reviewer_prompt(&self, methodology_line: &str, prompt: &str) -> String {
        let line = if self.line.is_empty() {
            String::new()
        } else {
            format!("\n{}", self.line)
        };
        format!("{methodology_line}{line}\n\n{prompt}")
    }
}

/// `/^(?:\/[A-Za-z0-9._\/-]+|[A-Za-z]:[\\/][A-Za-z0-9._~\\/-]+)$/`: a Unix
/// path, or a Windows one with its drive, which may carry the `~` of an 8.3
/// short name.
fn is_absolute_path(s: &str) -> bool {
    let b = s.as_bytes();
    let safe = |rest: &[u8], extra: &[u8]| {
        !rest.is_empty()
            && rest
                .iter()
                .all(|&c| c.is_ascii_alphanumeric() || b"._/-".contains(&c) || extra.contains(&c))
    };
    match b {
        [b'/', rest @ ..] => safe(rest, b""),
        [d, b':', b'/' | b'\\', rest @ ..] if d.is_ascii_alphabetic() => safe(rest, b"\\~"),
        _ => false,
    }
}

/// `/^[0-9a-f]{64}$/`.
fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// One `files` entry as the row records it: no text, no byte count. A field
/// the entry lacks is left out, as `JSON.stringify` drops `undefined`.
fn meta(f: &Value) -> Result<Value, String> {
    if f.is_null() {
        return Err(js_ext::null_property_error("path"));
    }
    let mut m = Map::new();
    for k in ["path", "sha256", "scope"] {
        if let Some(v) = js::get(f, k) {
            m.insert(k.into(), js_ext::js_order(v));
        }
    }
    let matched = js::get(f, "matched");
    if js::truthy(matched) {
        m.insert("matched".into(), js_ext::js_order(matched.expect("truthy")));
    }
    Ok(Value::Object(m))
}

/// Ports `conventionsChannel`. `args` is the workflow's parsed args object
/// and `profile` the validated profile. The two refusals are `Err` with the
/// JavaScript message, as is the `TypeError` from a `null` entry in
/// `conventions.files` or `conventions.skipped`.
pub fn conventions_channel(args: &Value, profile: &Value) -> Result<ConventionsChannel, String> {
    if args.is_null() {
        return Err(js_ext::null_property_error("conventionsPath"));
    }
    if profile.is_null() {
        return Err(js_ext::null_property_error("conventionFiles"));
    }
    let path = match js::get(args, "conventionsPath") {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    let sha256 = match js::get(args, "conventionsSha256") {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    };
    let lists_files =
        matches!(js::get(profile, "conventionFiles"), Some(Value::Array(fs)) if !fs.is_empty());
    if lists_files && path.is_empty() {
        return Err("tribunal: the profile lists conventionFiles, but args carry no conventionsPath. SKILL.md runs load-conventions.mjs after the diff is fetched and passes conventionsPath, conventionsSha256 and conventions.".into());
    }
    if !path.is_empty() && (!is_absolute_path(path) || !is_sha256(sha256.unwrap_or_default())) {
        let got = Value::Array(vec![
            Value::String(path.into()),
            sha256.map_or(Value::Null, |s| Value::String(s.into())),
        ]);
        return Err(format!(
            "tribunal: args.conventionsPath must be an absolute path with its conventionsSha256, got {}",
            js_ext::json_stringify(Some(&got))
        ));
    }
    if path.is_empty() {
        return Ok(ConventionsChannel {
            manifest: None,
            line: String::new(),
        });
    }
    let c = js::get(args, "conventions").filter(|c| js::is_object_like(c));
    let field = |k: &str| c.and_then(|c| js::get(c, k));
    let files = match field("files") {
        Some(Value::Array(fs)) => Value::Array(fs.iter().map(meta).collect::<Result<_, _>>()?),
        _ => Value::Null,
    };
    let skipped = match field("skipped") {
        Some(Value::Array(fs)) => Value::Array(
            fs.iter()
                .map(|f| {
                    if f.is_null() {
                        return Err(js_ext::null_property_error("path"));
                    }
                    Ok(js::get(f, "path").map_or(Value::Null, js_ext::js_order))
                })
                .collect::<Result<_, _>>()?,
        ),
        _ => Value::Null,
    };
    let missing = match field("missing") {
        Some(m @ Value::Array(_)) => js_ext::js_order(m),
        _ => Value::Null,
    };
    let mut manifest = Map::new();
    manifest.insert(
        "sha256".into(),
        Value::String(sha256.expect("checked").into()),
    );
    manifest.insert(
        "ref".into(),
        match field("ref") {
            Some(r @ Value::String(_)) => r.clone(),
            _ => Value::Null,
        },
    );
    manifest.insert("files".into(), files);
    manifest.insert("skipped".into(), skipped);
    manifest.insert("missing".into(), missing);
    Ok(ConventionsChannel {
        manifest: Some(Value::Object(manifest)),
        line: format!("CONVENTIONS FILE: {path}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn no_conventions_changes_no_prompt() {
        let w = conventions_channel(&json!({}), &json!({})).unwrap();
        assert_eq!(w.manifest, None);
        assert_eq!(w.with_conventions("REFUTE"), "REFUTE");
        assert_eq!(w.reviewer_prompt("M", "P"), "M\n\nP");
    }

    #[test]
    fn absolute_paths_on_unix_and_windows() {
        for ok in [
            "/tmp/c.md",
            "C:/Users/dev/AppData/Local/Temp/c.md",
            "C:\\Users\\dev\\AppData\\Local\\Temp\\c.md",
            "C:\\Users\\dev\\Temp/tribunal-conventions-a-b-1.md",
            "C:\\Users\\RUNNER~1\\AppData\\Local\\Temp/tribunal-conventions-a-b-1.md",
        ] {
            assert!(is_absolute_path(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "c.md",
            "\\tmp\\c.md",
            "C:c.md",
            "C:/",
            "/tmp/x$(id).md",
            "C:/tmp/x$(id).md",
            "/tmp\\c.md",
            "~/c.md",
            "/tmp/~x/c.md",
        ] {
            assert!(!is_absolute_path(bad), "{bad}");
        }
    }

    #[test]
    fn a_loaded_file_adds_its_line() {
        let args = json!({"conventionsPath": "/tmp/c.md", "conventionsSha256": SHA});
        let w = conventions_channel(&args, &json!({"conventionFiles": ["CLAUDE.md"]})).unwrap();
        assert_eq!(w.with_conventions("R"), "CONVENTIONS FILE: /tmp/c.md\n\nR");
        assert_eq!(
            w.reviewer_prompt("M", "P"),
            "M\nCONVENTIONS FILE: /tmp/c.md\n\nP"
        );
        assert_eq!(w.manifest.unwrap()["files"], Value::Null);
    }

    #[test]
    fn a_listed_file_without_a_path_is_refused() {
        let err = conventions_channel(&json!({}), &json!({"conventionFiles": ["CLAUDE.md"]}))
            .unwrap_err();
        assert!(err.contains("args carry no conventionsPath"));
        let err =
            conventions_channel(&json!({"conventionsPath": "rel.md"}), &json!({})).unwrap_err();
        assert!(err.ends_with("got [\"rel.md\",null]"));
    }
}
