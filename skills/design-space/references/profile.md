# Consumer profile

`.design-space.json` sits at the consuming repository's root and tells the skill where the design
space is. Bootstrap writes it (see [bootstrap.md](bootstrap.md)); commit it with the repository.

## Version 1 contract

Required fields:

- `schemaVersion`: exactly `1`.
- `layout`: exactly `"app"`, the design space app that bootstrap generates, kept in its own folder.
- `start.url`: an HTTP or HTTPS loopback URL using `localhost`, `127.0.0.0/8`, or `::1`.
- `paths.roots`: a non-empty array of repository-relative paths that bound design space edits,
  normally just the design space folder.

Optional fields:

- `name`: the product's name, a non-empty string. It titles the sidebar and the home page; when
  absent the package name is used, made readable.
- `start.command`: the local start command, normally `npm run design-space:dev`.
- `baseBranch`: the branch prototypes start from and open their pull request against. When it is
  absent, the step scripts use the remote's default branch. Set it when designers should branch
  from somewhere else, such as `development`.
- `paths.tokenSource`: the product's token file, shown on the token pages through
  `src/product-tokens.ts` (see [tokens.md](tokens.md)). Engineer-owned.
- `paths.components`: the product's design system folder; one specimen per component file in it;
  read by the coverage audit (see [components.md](components.md)). Engineer-owned. Bootstrap sets
  it from `--components`.
- `feedback.provider`: the visual feedback tool, `agentation`. `feedback.endpoint`, when present, is
  a loopback HTTP or HTTPS URL.

Every configured path must already exist, remain inside the repository after resolving symlinks,
and use `/`-separated repository-relative syntax. Omit what the repository does not have instead of
adding dummy values. The validator checks configuration only and never runs a command.

## Example

```json
{
  "schemaVersion": 1,
  "layout": "app",
  "start": { "command": "npm run design-space:dev", "url": "http://127.0.0.1:4100/" },
  "paths": { "roots": ["design-space"], "tokenSource": "src/styles/tokens.css" },
  "feedback": { "provider": "agentation", "endpoint": "http://127.0.0.1:4747" }
}
```

A profile without `"layout": "app"` is not supported: explain that the design space must be
bootstrapped with this skill, and stop. Run `node <skill directory>/scripts/validate-profile.mjs
<repository>/.design-space.json` (both paths absolute) after any change, last, because every
configured path must exist.
