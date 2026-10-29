# Security policy

## Reporting a vulnerability

Please report security issues privately, through GitHub's
[private vulnerability reporting](https://github.com/xmartlabs/agent-skills/security/advisories/new)
(the **Report a vulnerability** button under this repository's **Security**
tab). Do not open a public issue for them.

Include what is affected (a skill, a plugin, the CI), how to reproduce it, and
what an attacker gains. We will acknowledge the report and keep you posted
while we work on a fix.

## What is supported

Only the latest version of each tool gets fixes: the newest `tribunal/vN`
freeze and the `tribunal-rust-v*` binary it pins in
`plugins/tribunal/rust/release.json`, and the current `main` for everything
else.

## Things that work as designed

Some tools here run AI agents with shell access on your machine. That is how
they work, not a vulnerability, but it shapes how to use them safely: read
[tribunal's Security section](plugins/tribunal/README.md#security) before
reviewing a pull request from someone you do not trust.
