//! Port of `lib/operator.mjs`: the per-machine settings file
//! (`${REVIEW_HARNESS_CONFIG:-~/.claude/review-harness.json}`) and the repo a
//! review runs against when the caller names none.
//!
//! It also holds the Node semantics the loader scripts share and that no
//! shared module has: `JSON.parse` with V8's error messages, the messages of
//! `fs.readFileSync` / `fs.writeFileSync` errors, `path.join`, and the
//! `flag()` reading of argv every script uses. The scripts print those
//! messages to the person running the review, so they are reproduced, not
//! paraphrased.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;

use serde_json::{Map, Number, Value};
use tribunal_core::js;
use tribunal_core::profile::js_ext;

use crate::exec::Tools;

/// What a script would print and exit with. `stderr` only carries the reason
/// for a crash (a script that throws prints a stack, not a JSON line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl CliOutput {
    /// `process.stdout.write(JSON.stringify(out) + "\n"); process.exit(out.ok ? 0 : 1)`.
    #[must_use]
    pub fn json_line(out: &Map<String, Value>) -> Self {
        let ok = js::truthy(out.get("ok"));
        Self {
            stdout: format!(
                "{}\n",
                js_ext::json_stringify(Some(&Value::Object(out.clone())))
            ),
            stderr: String::new(),
            code: if ok { 0 } else { 1 },
        }
    }

    /// An uncaught exception: nothing on stdout, exit 1.
    #[must_use]
    pub fn crash(message: String) -> Self {
        Self {
            stdout: String::new(),
            stderr: message,
            code: 1,
        }
    }
}

/// The scripts' `flag(name)`: the argument after the first `name`, unless it
/// is missing, empty or itself starts with `--`.
#[must_use]
pub fn flag(argv: &[String], name: &str) -> String {
    match argv.iter().position(|a| a == name) {
        Some(i) => match argv.get(i + 1) {
            Some(v) if !v.is_empty() && !v.starts_with("--") => v.clone(),
            _ => String::new(),
        },
        None => String::new(),
    }
}

/// `/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/`.
#[must_use]
pub fn is_repo(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
    };
    matches!(s.split_once('/'), Some((o, n)) if part(o) && part(n))
}

// ---------------------------------------------------------------------------
// Node's path.posix.join

/// `path.posix.normalize`.
#[must_use]
pub fn path_normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let absolute = p.starts_with('/');
    let trailing = p.ends_with('/');
    let mut segs: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if segs.last().is_some_and(|s| *s != "..") {
                    segs.pop();
                } else if !absolute {
                    segs.push("..");
                }
            }
            s => segs.push(s),
        }
    }
    let mut out = segs.join("/");
    if out.is_empty() && !absolute {
        out = ".".into();
    }
    if !out.is_empty() && trailing {
        out.push('/');
    }
    if absolute {
        format!("/{out}")
    } else {
        out
    }
}

/// `path.posix.join`: the non-empty parts joined with `/`, normalized.
#[must_use]
pub fn path_join(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if joined.is_empty() {
        return ".".into();
    }
    path_normalize(&joined.join("/"))
}

// ---------------------------------------------------------------------------
// fs errors as Node words them

fn errno_of(e: &std::io::Error) -> Option<(&'static str, &'static str)> {
    Some(match e.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied if e.raw_os_error() == Some(1) => {
            ("EPERM", "operation not permitted")
        }
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        // ErrorKind::FilesystemLoop is unstable: match ELOOP by number.
        _ if e.raw_os_error() == Some(if cfg!(target_os = "linux") { 40 } else { 62 }) => {
            ("ELOOP", "too many symbolic links encountered")
        }
        ErrorKind::InvalidFilename => ("ENAMETOOLONG", "name too long"),
        ErrorKind::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        ErrorKind::StorageFull => ("ENOSPC", "no space left on device"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        _ => return None,
    })
}

/// The `message` of a Node fs error: `ENOENT: no such file or directory, open
/// '<path>'`. A failed `read` names no path. An errno with no entry above
/// falls back to Rust's own wording.
#[must_use]
pub fn node_fs_message(e: &std::io::Error, syscall: &str, path: Option<&str>) -> String {
    let (code, desc) = match errno_of(e) {
        Some(x) => x,
        None => return format!("{e}, {syscall}"),
    };
    match path {
        Some(p) => format!("{code}: {desc}, {syscall} '{p}'"),
        None => format!("{code}: {desc}, {syscall}"),
    }
}

/// `fs.readFileSync(path, "utf8")`: invalid UTF-8 becomes U+FFFD, as Node's
/// decoder does.
///
/// # Errors
/// Node's message for the failed `open` or `read`.
pub fn read_file_utf8(path: &str) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| node_fs_message(&e, "open", Some(path)))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| node_fs_message(&e, "read", None))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `fs.writeFileSync(path, text)`.
///
/// # Errors
/// Node's message for the failed `open` or `write`.
pub fn write_file(path: &str, text: &str) -> Result<(), String> {
    let mut f = std::fs::File::create(path).map_err(|e| node_fs_message(&e, "open", Some(path)))?;
    f.write_all(text.as_bytes())
        .map_err(|e| node_fs_message(&e, "write", None))
}

// ---------------------------------------------------------------------------
// JSON.parse with V8's messages

/// `JSON.parse(text)`. The value comes back with its keys in source order
/// (a repeated key keeps its first position and its last value, as in V8);
/// [`js_ext::json_stringify`] applies JavaScript's own key order on output.
///
/// # Errors
/// V8's `SyntaxError` message, word for word (Node 24).
pub fn json_parse(text: &str) -> Result<Value, String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    JsonParser { s: &units, i: 0 }.parse()
}

struct JsonParser<'a> {
    s: &'a [u16],
    i: usize,
}

enum Frame {
    Arr(Vec<Value>),
    Obj(Map<String, Value>, String),
}

const SPECIAL_SOURCES: [&str; 4] = ["[object Object]", "undefined", "Infinity", "NaN"];
const CONTEXT: usize = 10;

impl JsonParser<'_> {
    fn peek(&self) -> Option<u16> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0A | 0x0D)) {
            self.i += 1;
        }
    }

    /// V8 counts `\r\n` as one line break, and `\r` or `\n` alone as one.
    fn location(&self, pos: usize) -> (usize, usize) {
        let mut line = 1;
        let mut last_break = 0;
        let mut k = 0;
        while k < pos {
            if self.s[k] == 0x0D && k + 1 < pos && self.s[k + 1] == 0x0A {
                k += 1;
            }
            if self.s[k] == 0x0D || self.s[k] == 0x0A {
                line += 1;
                last_break = k + 1;
            }
            k += 1;
        }
        (line, 1 + k - last_break)
    }

    fn at(&self, msg: &str, pos: usize) -> String {
        let (line, column) = self.location(pos);
        format!("{msg} in JSON at position {pos} (line {line} column {column})")
    }

    fn text(&self, a: usize, b: usize) -> String {
        String::from_utf16_lossy(&self.s[a..b.min(self.s.len())])
    }

    /// `ReportUnexpectedToken` with no explicit message: the token at `pos`
    /// decides the wording.
    fn unexpected(&self, pos: usize) -> String {
        let Some(&c) = self.s.get(pos) else {
            return "Unexpected end of JSON input".into();
        };
        if c == u16::from(b'"') {
            return self.at("Unexpected string", pos);
        }
        if c == u16::from(b'-') || (0x30..=0x39).contains(&c) {
            return self.at("Unexpected number", pos);
        }
        let source = self.text(0, self.s.len());
        if SPECIAL_SOURCES.contains(&source.as_str()) {
            return format!("\"{source}\" is not valid JSON");
        }
        let token = String::from_utf16_lossy(&[c]);
        let len = self.s.len();
        if len <= 2 * CONTEXT {
            format!("Unexpected token '{token}', \"{source}\" is not valid JSON")
        } else if pos < CONTEXT {
            format!(
                "Unexpected token '{token}', \"{}\"... is not valid JSON",
                self.text(0, pos + CONTEXT)
            )
        } else if pos < len - CONTEXT {
            format!(
                "Unexpected token '{token}', ...\"{}\"... is not valid JSON",
                self.text(pos - CONTEXT, pos + CONTEXT)
            )
        } else {
            format!(
                "Unexpected token '{token}', ...\"{}\" is not valid JSON",
                self.text(pos - CONTEXT, len)
            )
        }
    }

    fn digit_at(&self, k: usize) -> bool {
        matches!(self.s.get(k), Some(c) if (0x30..=0x39).contains(c))
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out: Vec<u16> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.at("Unterminated string", self.s.len()));
            };
            if c == u16::from(b'"') {
                self.i += 1;
                // A lone surrogate cannot live in a Rust String: it becomes
                // U+FFFD (a declared difference).
                return Ok(String::from_utf16_lossy(&out));
            }
            if c == u16::from(b'\\') {
                self.i += 1;
                let Some(e) = self.peek() else {
                    return Err(self.unexpected(self.i));
                };
                // V8 reports a character past Latin-1 after `\` as an
                // unexpected token, not as a bad escape.
                if e > 0xFF {
                    return Err(self.unexpected(self.i));
                }
                let unit = match e {
                    0x22 | 0x5C | 0x2F => e,
                    0x62 => 0x08,
                    0x66 => 0x0C,
                    0x6E => 0x0A,
                    0x72 => 0x0D,
                    0x74 => 0x09,
                    0x75 => {
                        let mut v: u16 = 0;
                        for k in 1..=4 {
                            let d = self
                                .s
                                .get(self.i + k)
                                .and_then(|&h| char::from_u32(u32::from(h)))
                                .and_then(|h| h.to_digit(16));
                            match d {
                                Some(d) => v = v * 16 + d as u16,
                                None => return Err(self.at("Bad Unicode escape", self.i + k)),
                            }
                        }
                        self.i += 4;
                        v
                    }
                    _ => return Err(self.at("Bad escaped character", self.i)),
                };
                out.push(unit);
                self.i += 1;
                continue;
            }
            if c < 0x20 {
                return Err(self.at("Bad control character in string literal", self.i));
            }
            out.push(c);
            self.i += 1;
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.peek() == Some(u16::from(b'-')) {
            self.i += 1;
        }
        if self.peek() == Some(u16::from(b'0')) {
            self.i += 1;
            if self.digit_at(self.i) {
                return Err(self.unexpected(self.i));
            }
        } else {
            if !self.digit_at(self.i) {
                return Err(self.at("No number after minus sign", self.i));
            }
            while self.digit_at(self.i) {
                self.i += 1;
            }
        }
        if self.peek() == Some(u16::from(b'.')) {
            self.i += 1;
            if !self.digit_at(self.i) {
                return Err(self.at("Unterminated fractional number", self.i));
            }
            while self.digit_at(self.i) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(0x65 | 0x45)) {
            self.i += 1;
            if matches!(self.peek(), Some(0x2B | 0x2D)) {
                self.i += 1;
            }
            if !self.digit_at(self.i) {
                return Err(self.at("Exponent part is missing a number", self.i));
            }
            while self.digit_at(self.i) {
                self.i += 1;
            }
        }
        let x: f64 = self.text(start, self.i).parse().unwrap_or(f64::NAN);
        // A literal beyond f64 range is Infinity in JavaScript, which
        // JSON.stringify prints as null; serde_json cannot hold it.
        Ok(Number::from_f64(x).map_or(Value::Null, Value::Number))
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
        for (k, w) in word.bytes().enumerate() {
            match self.s.get(self.i + k) {
                None => return Err(self.unexpected(self.i + k)),
                Some(&c) if c != u16::from(w) => return Err(self.unexpected(self.i + k)),
                Some(_) => {}
            }
        }
        self.i += word.len();
        Ok(v)
    }

    fn colon(&mut self) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(u16::from(b':')) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.at("Expected ':' after property name", self.i))
        }
    }

    fn parse(mut self) -> Result<Value, String> {
        let mut stack: Vec<Frame> = Vec::new();
        'value: loop {
            self.skip_ws();
            let mut v = match self.peek() {
                Some(0x22) => Value::String(self.string()?),
                Some(c) if c == 0x2D || (0x30..=0x39).contains(&c) => self.number()?,
                Some(0x7B) => {
                    self.i += 1;
                    self.skip_ws();
                    match self.peek() {
                        Some(0x7D) => {
                            self.i += 1;
                            Value::Object(Map::new())
                        }
                        Some(0x22) => {
                            let k = self.string()?;
                            self.colon()?;
                            stack.push(Frame::Obj(Map::new(), k));
                            continue 'value;
                        }
                        _ => return Err(self.at("Expected property name or '}'", self.i)),
                    }
                }
                Some(0x5B) => {
                    self.i += 1;
                    self.skip_ws();
                    if self.peek() == Some(0x5D) {
                        self.i += 1;
                        Value::Array(Vec::new())
                    } else {
                        stack.push(Frame::Arr(Vec::new()));
                        continue 'value;
                    }
                }
                Some(0x74) => self.literal("true", Value::Bool(true))?,
                Some(0x66) => self.literal("false", Value::Bool(false))?,
                Some(0x6E) => self.literal("null", Value::Null)?,
                _ => return Err(self.unexpected(self.i)),
            };
            loop {
                match stack.last_mut() {
                    None => {
                        self.skip_ws();
                        if self.i < self.s.len() {
                            // The one message that says "at position" without "in JSON".
                            let (line, column) = self.location(self.i);
                            return Err(format!(
                                "Unexpected non-whitespace character after JSON at position {} (line {line} column {column})",
                                self.i
                            ));
                        }
                        return Ok(v);
                    }
                    Some(Frame::Arr(items)) => {
                        items.push(v);
                        self.skip_ws();
                        match self.peek() {
                            Some(0x2C) => {
                                self.i += 1;
                                continue 'value;
                            }
                            Some(0x5D) => {
                                self.i += 1;
                                let Some(Frame::Arr(items)) = stack.pop() else {
                                    unreachable!("the frame just matched")
                                };
                                v = Value::Array(items);
                            }
                            _ => {
                                return Err(
                                    self.at("Expected ',' or ']' after array element", self.i)
                                )
                            }
                        }
                    }
                    Some(Frame::Obj(map, key)) => {
                        map.insert(std::mem::take(key), v);
                        self.skip_ws();
                        match self.peek() {
                            Some(0x2C) => {
                                self.i += 1;
                                self.skip_ws();
                                if self.peek() != Some(0x22) {
                                    return Err(
                                        self.at("Expected double-quoted property name", self.i)
                                    );
                                }
                                let k = self.string()?;
                                self.colon()?;
                                if let Some(Frame::Obj(_, key)) = stack.last_mut() {
                                    *key = k;
                                }
                                continue 'value;
                            }
                            Some(0x7D) => {
                                self.i += 1;
                                let Some(Frame::Obj(map, _)) = stack.pop() else {
                                    unreachable!("the frame just matched")
                                };
                                v = Value::Object(map);
                            }
                            _ => {
                                return Err(
                                    self.at("Expected ',' or '}' after property value", self.i)
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// operator.mjs

/// `operatorConfigPath`: `REVIEW_HARNESS_CONFIG` when set and non-empty,
/// else `<home>/.claude/review-harness.json`. `home` is what `os.homedir()`
/// returns (`$HOME`); a test passes a scratch directory.
#[must_use]
pub fn operator_config_path(review_harness_config: Option<&str>, home: &str) -> String {
    match review_harness_config {
        Some(p) if !p.is_empty() => p.to_owned(),
        _ => path_join(&[home, ".claude", "review-harness.json"]),
    }
}

/// `readOperatorConfig`: `error` is set when the file exists but cannot be
/// used; `config` is then empty.
#[derive(Debug, Clone, PartialEq)]
pub struct OperatorConfig {
    pub path: String,
    pub config: Map<String, Value>,
    pub present: bool,
    pub error: Option<String>,
}

/// `readOperatorConfig(path)`.
#[must_use]
pub fn read_operator_config(path: &str) -> OperatorConfig {
    let fail = |error: String| OperatorConfig {
        path: path.to_owned(),
        config: Map::new(),
        present: false,
        error: Some(error),
    };
    // existsSync: false for "" and for anything stat cannot reach.
    if path.is_empty() || !Path::new(path).exists() {
        return OperatorConfig {
            path: path.to_owned(),
            config: Map::new(),
            present: false,
            error: None,
        };
    }
    let parsed = read_file_utf8(path).and_then(|t| json_parse(&t));
    let config = match parsed {
        Ok(Value::Object(m)) => m,
        Ok(_) => return fail(format!("{path} must hold a JSON object")),
        Err(e) => return fail(format!("{path} is not valid JSON: {e}")),
    };
    if let Some(dr) = config.get("defaultRepo") {
        if !is_repo(&js_ext::to_string(Some(dr))) {
            return fail(format!(
                "{path}: defaultRepo must be owner/name, got {}",
                js_ext::json_stringify(Some(dr))
            ));
        }
    }
    OperatorConfig {
        path: path.to_owned(),
        config,
        present: true,
        error: None,
    }
}

fn is_repo_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// `repoFromRemoteUrl`: `owner/name` of a GitHub remote URL, or `None`.
/// Ports `/github\.com[:/]([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\.git)?\/?$/`
/// on the trimmed URL: the first `github.com` from which the rest matches.
#[must_use]
pub fn repo_from_remote_url(url: &str) -> Option<String> {
    let s = js::trim(url);
    let mut from = 0;
    while let Some(off) = s[from..].find("github.com") {
        let at = from + off;
        from = at + 1;
        let rest = &s[at + "github.com".len()..];
        let Some(rest) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('/')) else {
            continue;
        };
        let Some((owner, tail)) = rest.split_once('/') else {
            continue;
        };
        if owner.is_empty() || !owner.chars().all(is_repo_char) {
            continue;
        }
        let tail = tail.strip_suffix('/').unwrap_or(tail);
        if tail.is_empty() || !tail.chars().all(is_repo_char) {
            continue;
        }
        // The name is lazy: a trailing `.git` is left out when a name remains.
        let name = match tail.strip_suffix(".git") {
            Some(n) if !n.is_empty() => n,
            _ => tail,
        };
        return Some(format!("{owner}/{name}"));
    }
    None
}

/// `cwdRepo`: the `origin` remote of `cwd`, through `git -C <cwd> remote
/// get-url origin`. Any failure is `None`.
pub async fn cwd_repo(tools: &Tools, cwd: &Path) -> Option<String> {
    let dir = cwd.to_string_lossy();
    let out = tools
        .git(&["-C", &dir, "remote", "get-url", "origin"], None)
        .await
        .ok()?;
    if !out.ok() {
        return None;
    }
    repo_from_remote_url(&out.stdout)
}

/// The repo a run reviews and where that came from (`flag`,
/// `operator:<path>` or `cwd-origin`). `repo` is a JSON value because an
/// operator's `defaultRepo` is used as written once its `String()` passes
/// the check, so `["acme/widgets"]` is accepted as it is in JavaScript.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRepo {
    pub repo: Value,
    pub source: String,
}

/// `resolveRepo`: `--repo`, else the operator's `defaultRepo`, else the
/// `origin` of `cwd`.
///
/// # Errors
/// The script's `error` text.
pub async fn resolve_repo(
    tools: &Tools,
    flag: &str,
    operator: &OperatorConfig,
    cwd: &Path,
) -> Result<ResolvedRepo, String> {
    if !flag.is_empty() {
        return if is_repo(flag) {
            Ok(ResolvedRepo {
                repo: Value::String(flag.to_owned()),
                source: "flag".into(),
            })
        } else {
            Err(format!(
                "--repo must be owner/name, got {}",
                js_ext::json_stringify(Some(&Value::String(flag.to_owned())))
            ))
        };
    }
    if let Some(e) = &operator.error {
        return Err(e.clone());
    }
    if let Some(dr) = operator
        .config
        .get("defaultRepo")
        .filter(|v| js::truthy(Some(v)))
    {
        return Ok(ResolvedRepo {
            repo: dr.clone(),
            source: format!("operator:{}", operator.path),
        });
    }
    if let Some(repo) = cwd_repo(tools, cwd).await {
        return Ok(ResolvedRepo {
            repo: Value::String(repo),
            source: "cwd-origin".into(),
        });
    }
    Err(format!(
        "no repo to review: pass --repo <owner/name>, set \"defaultRepo\" in {}, or run from a checkout whose origin is on GitHub",
        operator.path
    ))
}

/// The process facts a script reads besides its argv: `$REVIEW_HARNESS_CONFIG`,
/// `$HOME` and the working directory. Injected so a test never reads the
/// real home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorEnv {
    pub review_harness_config: Option<String>,
    pub home: String,
    pub cwd: std::path::PathBuf,
}

impl OperatorEnv {
    /// The settings file this environment names.
    #[must_use]
    pub fn config_path(&self) -> String {
        operator_config_path(self.review_harness_config.as_deref(), &self.home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls() {
        assert_eq!(
            repo_from_remote_url("git@github.com:acme/widgets.git\n").as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            repo_from_remote_url("https://github.com/acme/.git").as_deref(),
            Some("acme/.git")
        );
        assert_eq!(repo_from_remote_url("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn v8_messages() {
        assert_eq!(json_parse("").unwrap_err(), "Unexpected end of JSON input");
        assert_eq!(
            json_parse("{\"a\":1,}").unwrap_err(),
            "Expected double-quoted property name in JSON at position 7 (line 1 column 8)"
        );
        assert_eq!(json_parse("NaN").unwrap_err(), "\"NaN\" is not valid JSON");
    }

    #[test]
    fn join_normalizes() {
        assert_eq!(
            path_join(&["/a/b/", ".claude", "x.json"]),
            "/a/b/.claude/x.json"
        );
        assert_eq!(path_join(&["", ".claude", "x"]), ".claude/x");
        assert_eq!(path_join(&["a/../..", ".claude"]), "../.claude");
    }
}
