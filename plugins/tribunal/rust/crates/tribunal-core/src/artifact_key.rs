//! Port of the ARTIFACT-KEY region: the file name an external head writes its
//! vote under, `<slot>[r<round>]-<source>`.

/// `slot_id` is the finding's panel index; a clean-sweep finding has none and
/// keys as `x`. Round 0 and no round both mean round 0, as JavaScript's
/// truthiness had it.
pub fn artifact_key(slot_id: Option<u64>, source: &str, round: Option<u64>) -> String {
    let slot = slot_id.map_or_else(|| "x".to_owned(), |s| s.to_string());
    let round = match round {
        Some(r) if r != 0 => format!("r{r}"),
        _ => String::new(),
    };
    format!("{slot}{round}-{source}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_scheme() {
        assert_eq!(artifact_key(Some(0), "codex", None), "0-codex");
        assert_eq!(artifact_key(Some(3), "gemini", Some(1)), "3r1-gemini");
        assert_eq!(artifact_key(None, "codex", Some(0)), "x-codex");
    }
}
