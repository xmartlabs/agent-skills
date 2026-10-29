//! Port of `skills/review-pr/scripts/harness-version.mjs`: a content hash of
//! the files that can change what a review does, and the separate hash of
//! the eval battery.
//!
//! Each file is hashed as bytes; the id is the first 12 hex digits of the
//! SHA-256 of the manifest (`path:sha` lines joined by `\n`). Paths sort by
//! UTF-16 code units, as `Array.prototype.sort` does, and a symlink counts as
//! a file whatever it points at, as a `Dirent` reports it.

use std::path::Path;

use crate::import_adjacency::posix_join;

pub const SURFACE: &[&str] = &["skills/review-pr/SKILL.md"];

pub const SURFACE_DIRS: &[(&str, &[&str])] = &[
    ("skills/review-pr/references", &[".md"]),
    ("skills/review-pr/scripts", &[".mjs"]),
    ("workflows", &[".js"]),
    ("lib", &[".mjs", ".sh"]),
    ("agents", &[".md"]),
    ("methodology", &[".md"]),
];

pub const BATTERY_DIR: &str = "skills/review-pr/eval";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub id: String,
    pub entries: Vec<Entry>,
    pub manifest: String,
}

/// `PLUGIN_ROOT`: `CLAUDE_PLUGIN_ROOT` when set and non-empty, else the
/// plugin directory the script sits in.
pub fn plugin_root(env: Option<&str>, own_root: &str) -> String {
    env.filter(|v| !v.is_empty()).unwrap_or(own_root).to_owned()
}

fn utf16_sort(v: &mut [String]) {
    v.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
}

fn files_under(root: &str, dir: &str, exts: Option<&[&str]>) -> Result<Vec<String>, String> {
    let abs = posix_join(&[root, dir]);
    if !Path::new(&abs).exists() {
        return Err(format!("harness surface directory missing: {dir}"));
    }
    let mut entries: Vec<(String, bool)> = std::fs::read_dir(&abs)
        .map_err(|e| e.to_string())?
        .map(|e| {
            let e = e.map_err(|e| e.to_string())?;
            let is_dir = e.file_type().map_err(|e| e.to_string())?.is_dir();
            Ok((e.file_name().to_string_lossy().into_owned(), is_dir))
        })
        .collect::<Result<_, String>>()?;
    entries.sort();
    let mut out = Vec::new();
    for (name, is_dir) in entries {
        let rel = format!("{dir}/{name}");
        if is_dir {
            out.extend(files_under(root, &rel, exts)?);
        } else if exts.is_none_or(|exts| exts.iter().any(|e| name.ends_with(e))) {
            out.push(rel);
        }
    }
    Ok(out)
}

fn digest(root: &str, mut paths: Vec<String>) -> Result<(Vec<Entry>, String, String), String> {
    utf16_sort(&mut paths);
    let entries = paths
        .into_iter()
        .map(|rel| {
            let abs = posix_join(&[root, &rel]);
            if !Path::new(&abs).exists() {
                return Err(format!("harness surface file missing: {rel}"));
            }
            let bytes = std::fs::read(&abs).map_err(|e| e.to_string())?;
            Ok(Entry {
                sha: sha256_hex(&bytes),
                path: rel,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let manifest = entries
        .iter()
        .map(|e| format!("{}:{}", e.path, e.sha))
        .collect::<Vec<_>>()
        .join("\n");
    let short = sha256_hex(manifest.as_bytes())[..12].to_owned();
    Ok((entries, manifest, short))
}

/// `surfaceFiles()`.
///
/// # Errors
/// The script's throw for a missing surface directory, or an I/O error
/// reading one.
pub fn surface_files(root: &str) -> Result<Vec<String>, String> {
    let mut files: Vec<String> = SURFACE.iter().map(|s| (*s).to_owned()).collect();
    for (dir, exts) in SURFACE_DIRS {
        files.extend(files_under(root, dir, Some(exts))?);
    }
    utf16_sort(&mut files);
    Ok(files)
}

/// `batteryFiles()`.
///
/// # Errors
/// As [`surface_files`], for the battery directory.
pub fn battery_files(root: &str) -> Result<Vec<String>, String> {
    let mut files = files_under(root, BATTERY_DIR, None)?;
    utf16_sort(&mut files);
    Ok(files)
}

/// `harnessVersion()`.
///
/// # Errors
/// Where the script throws: a surface directory or file missing, or one that
/// cannot be read.
pub fn harness_version(root: &str) -> Result<Version, String> {
    let (entries, manifest, short) = digest(root, surface_files(root)?)?;
    Ok(Version {
        id: format!("pr-harness-{short}"),
        entries,
        manifest,
    })
}

/// `batteryVersion()`.
///
/// # Errors
/// As [`harness_version`].
pub fn battery_version(root: &str) -> Result<Version, String> {
    let (entries, manifest, short) = digest(root, battery_files(root)?)?;
    Ok(Version {
        id: format!("pr-battery-{short}"),
        entries,
        manifest,
    })
}

/// The script's entry: its stdout for `[--short | --manifest]`.
///
/// # Errors
/// As [`harness_version`]; the script prints nothing and exits 1.
pub fn run(args: &[&str], root: &str) -> Result<String, String> {
    let v = harness_version(root)?;
    Ok(if args.contains(&"--short") {
        format!("{}\n", v.id)
    } else if args.contains(&"--manifest") {
        let mut out = format!("{}\n", v.id);
        for e in &v.entries {
            out.push_str(&format!("  {}  {}\n", &e.sha[..12], e.path));
        }
        out
    } else {
        format!("{}  ({} files)\n", v.id, v.entries.len())
    })
}

/// `createHash("sha256").update(bytes).digest("hex")`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::load_profile::sha256_hex_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn paths_sort_by_utf16_units() {
        let mut v = vec!["\u{ff5e}".to_owned(), "\u{1f600}".to_owned()];
        utf16_sort(&mut v);
        assert_eq!(v, ["\u{1f600}", "\u{ff5e}"]);
    }
}
