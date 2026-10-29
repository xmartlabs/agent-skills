//! `ENGINE_VERSION`: `rust-` and 12 hex digits of an FNV-1a 64 hash over
//! every source file of the workspace and the plugin scripts
//! `tribunal_context::typescript_fallback` embeds (path and bytes, in path
//! order), so a metrics row and a posted footer name the exact Rust build that
//! ran, the way `harnessVersion` names the plugin files.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("rs" | "txt" | "json" | "toml" | "mjs")
        ) {
            out.push(p);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let mut files = Vec::new();
    collect(&root.join("crates"), &mut files);
    collect(&root.join("schemas"), &mut files);
    files.push(root.join("Cargo.lock"));
    files.push(root.join("Cargo.toml"));
    // The scripts `tribunal_context::typescript_fallback` embeds.
    let embedded = ["attention-facts.mjs", "blast-radius.mjs", "relay.mjs"]
        .map(|f| root.join("../lib").join(f));
    files.extend(embedded.iter().cloned());
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f);
        feed(rel.to_string_lossy().replace('\\', "/").as_bytes());
        feed(&[0]);
        feed(&std::fs::read(f).unwrap_or_default());
        feed(&[0]);
    }
    println!(
        "cargo:rustc-env=TRIBUNAL_ENGINE_VERSION=rust-{:012x}",
        h >> 16
    );
    println!("cargo:rerun-if-changed={}", root.join("crates").display());
    println!("cargo:rerun-if-changed={}", root.join("schemas").display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("Cargo.lock").display()
    );
    for f in &embedded {
        println!("cargo:rerun-if-changed={}", f.display());
    }
}
