//! Port of `lib/section-diff.mjs`: cuts the PR's patch on disk into one file
//! per changed file, an excerpt (`.slice`) per section and one for the whole
//! diff, and returns the index the script relays.
//!
//! The cut is DIFF-USABLE and DIFF-SECTIONING from `tribunal_core::lens`, the
//! same regions the script slices out of the workflow. Lengths and the excerpt
//! cut count UTF-16 units; a cut through a surrogate pair writes U+FFFD, as
//! Node's `writeFileSync` does with the lone half.

use std::path::Path;

use serde_json::{json, Value};
use tribunal_core::evidence::js_ext::number_value;
use tribunal_core::js;
use tribunal_core::lens::diff_sectioning::{added_ranges, split_diff_by_file};
use tribunal_core::lens::diff_usable::diff_is_usable;
use tribunal_core::lens::js_ext::utf16_prefix_lossy;

use crate::import_adjacency::posix_join;
use crate::incremental_scope::read_file_utf8;

pub const SLICE_CHARS: usize = 6000;

/// Ports `sliceFor`: the external heads' excerpt of one section.
pub fn slice_for(file: &str, own: &str) -> String {
    if js::utf16_len(own) <= SLICE_CHARS {
        format!("Diff for {file} (complete):\n{own}")
    } else {
        format!(
            "Diff for {file} (truncated to {SLICE_CHARS} chars):\n{}",
            utf16_prefix_lossy(own, SLICE_CHARS)
        )
    }
}

/// Ports `wholeSliceFor`.
pub fn whole_slice_for(diff: &str) -> String {
    format!(
        "Diff (truncated to {SLICE_CHARS} chars):\n{}",
        utf16_prefix_lossy(diff, SLICE_CHARS)
    )
}

/// `Number(arg) || 0`, how the script reads its `<additions>` and
/// `<deletions>` arguments. `None` is a missing argument.
pub fn count_arg(arg: Option<&str>) -> f64 {
    let n = arg.map_or(f64::NAN, js::string_to_number);
    if n == 0.0 || n.is_nan() {
        0.0
    } else {
        n
    }
}

fn write(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("cannot write {path}: {e}"))
}

/// Ports `sectionDiff`. `out_dir` is created even when the diff is not usable,
/// and the paths in the index are `path.join(out_dir, …)`.
///
/// # Errors
/// Where the script throws: `out_dir` cannot be created or a file in it
/// cannot be written.
pub fn section_diff(
    diff: &str,
    out_dir: &str,
    additions: f64,
    deletions: f64,
) -> Result<Value, String> {
    let usable = diff_is_usable(Some(diff), Some(additions), Some(deletions));
    std::fs::create_dir_all(out_dir).map_err(|e| format!("cannot create {out_dir}: {e}"))?;
    let mut index = json!({
        "usable": usable,
        "diffChars": js::utf16_len(diff),
        "unparsed": [],
        "sections": [],
        "wholeSlice": null,
    });
    if !usable {
        return Ok(index);
    }
    let split = split_diff_by_file(Some(diff));
    index["unparsed"] = json!(split.unparsed);
    let mut sections = Vec::with_capacity(split.sections.len());
    for (i, (file, text)) in split.sections.iter().enumerate() {
        let path = posix_join(&[out_dir, &format!("{i}.patch")]);
        let slice = posix_join(&[out_dir, &format!("{i}.slice")]);
        write(&path, text)?;
        write(&slice, &slice_for(file, text))?;
        let added: Vec<Value> = added_ranges(Some(text))
            .into_iter()
            .map(|[a, b]| json!([number_value(a), number_value(b)]))
            .collect();
        sections.push(json!({
            "file": file,
            "path": path,
            "chars": js::utf16_len(text),
            "slice": slice,
            "added": added,
        }));
    }
    index["sections"] = Value::Array(sections);
    let whole = posix_join(&[out_dir, "whole.slice"]);
    write(&whole, &whole_slice_for(diff))?;
    index["wholeSlice"] = Value::String(whole);
    Ok(index)
}

/// The script's main path: `node section-diff.mjs <diff-file> <out-dir>
/// <additions> <deletions>`, returning the object it relays. Pass the counts
/// through [`count_arg`] when they come from the command line.
///
/// # Errors
/// The usage error (an empty `diff_path` or `out_dir`), an unreadable diff,
/// and whatever [`section_diff`] refuses. The script's unreadable-diff line
/// ends with Node's error message; this one names only the path.
pub fn run(
    diff_path: &str,
    out_dir: &str,
    additions: f64,
    deletions: f64,
) -> Result<Value, String> {
    if diff_path.is_empty() || out_dir.is_empty() {
        return Err(
            "usage: section-diff.mjs <diff-file> <out-dir> <additions> <deletions>".to_owned(),
        );
    }
    let diff = read_file_utf8(Path::new(diff_path))
        .ok_or_else(|| format!("FATAL: cannot read the diff at {diff_path}"))?;
    section_diff(&diff, out_dir, additions, deletions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_like_number_or_zero() {
        assert_eq!(count_arg(None), 0.0);
        assert_eq!(count_arg(Some("abc")), 0.0);
        assert_eq!(count_arg(Some(" 0x10 ")), 16.0);
        assert_eq!(count_arg(Some("-0")), 0.0);
    }

    #[test]
    fn an_excerpt_cut_inside_a_pair_ends_in_a_replacement() {
        let own = format!("{}😀", "a".repeat(SLICE_CHARS - 1));
        assert!(slice_for("x", &own).ends_with("a\u{FFFD}"));
        assert!(slice_for("x", "abc").ends_with("(complete):\nabc"));
    }
}
