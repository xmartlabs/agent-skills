//! Alone in its test binary: it changes this process's environment.

use std::process::Command;

use tribunal_context::exec::{take_conventions_token, ConventionsToken, CONVENTIONS_TOKEN_VAR};

#[test]
fn the_token_leaves_the_environment_so_no_child_inherits_it() {
    std::env::set_var(CONVENTIONS_TOKEN_VAR, "sentinel-docs-token");
    assert_eq!(
        take_conventions_token(),
        ConventionsToken::new("sentinel-docs-token")
    );
    assert_eq!(std::env::var_os(CONVENTIONS_TOKEN_VAR), None);
    let out = Command::new("env").output().unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sentinel-docs-token"));

    std::env::set_var(CONVENTIONS_TOKEN_VAR, "");
    assert_eq!(take_conventions_token(), None, "empty means not given");
    assert_eq!(std::env::var_os(CONVENTIONS_TOKEN_VAR), None);
}
