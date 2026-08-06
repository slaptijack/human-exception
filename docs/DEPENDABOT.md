# Dependency updates

Human Exception uses [Dependabot version
updates](https://docs.github.com/en/code-security/dependabot/dependabot-version-updates),
configured in
[`.github/dependabot.yml`](../.github/dependabot.yml), to keep two
things current:

- Cargo dependencies declared in `Cargo.toml`.
- The GitHub Actions used by `.github/workflows/`.

Both run on a weekly schedule. Compatible `minor` and `patch` updates
within each ecosystem are grouped into a single pull request to reduce
noise; `major` updates are left ungrouped so each one gets its own PR
and explicit review. Dependabot never merges anything itself — every
update lands as an ordinary pull request subject to the repository's
normal branch protection and required reviews.

## SHA-pinned GitHub Actions

Third-party GitHub Actions in this repository are pinned to immutable
commit SHAs rather than mutable tags (see
[`.github/workflows/release-plz.yml`](../.github/workflows/release-plz.yml)
for an example: `uses: release-plz/action@1a0c5be...# v0.5.131`). A
pinned SHA can't be silently repointed at different code the way a tag
like `v4` can, which is why the repo prefers it.

Dependabot understands this pattern natively: when it updates a
SHA-pinned action, it resolves the new release's commit and rewrites
both the SHA and the trailing `# vX.Y.Z` comment together. No special
configuration is needed to preserve the pinning policy, and Dependabot
pull requests for `github-actions` should always show a full SHA, never
a bare tag.

## One-time maintainer setup

Dependabot version updates start working as soon as
`.github/dependabot.yml` merges to `main` — no toggle is required for
that part. Two related settings are not controlled by the config file
and should be confirmed once, under repo Settings → Code security:

1. **Dependabot alerts** — on by default for public repositories.
   Confirm it's enabled.
2. **Dependabot security updates** — opt-in. Enable this so Dependabot
   also opens PRs automatically in response to security advisories
   affecting Cargo dependencies, independent of the weekly version-update
   schedule.

## CI on Dependabot pull requests

Dependabot pull requests are regular pull requests against branches in
this repository, so they trigger `.github/workflows/ci.yml` through its
existing `pull_request` trigger and are held to the same required
status checks as any contributor PR. No additional workflow wiring is
needed.

## Validating the configuration

To confirm the configuration is valid and producing real pull requests
without waiting for the weekly schedule, a maintainer can trigger an
on-demand check from the repository's **Insights → Dependency graph →
Dependabot** tab (or the equivalent "Check for updates" action in the
UI) after `.github/dependabot.yml` is live on `main`.
