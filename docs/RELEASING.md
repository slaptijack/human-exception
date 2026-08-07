# Releasing

Human Exception publishes to [crates.io](https://crates.io/) using
[release-plz](https://release-plz.dev/), automated through
[`.github/workflows/release-plz.yml`](../.github/workflows/release-plz.yml).

## Flow

1. A change merges to `main`.
2. The **Release-plz PR** job opens or updates a release pull request
   containing the version bump (in `Cargo.toml`) and the generated
   changelog (`CHANGELOG.md`), based on commits since the last
   release.
3. A maintainer reviews and merges the release PR like any other PR.
4. That merge triggers the **Release-plz release** job, which tags the
   release, publishes the crate to crates.io, and creates the matching
   GitHub release.

Both jobs are idempotent: the release job only publishes versions that
aren't already on crates.io, so re-running it (or the workflow
retriggering on an unrelated push) never double-publishes or
double-tags.

## Commit and PR title convention

release-plz determines the version bump and changelog entries from
[Conventional Commits](https://www.conventionalcommits.org/). Because
PRs in this repository are typically squash-merged, **the PR title
becomes the commit message release-plz reads** — so title PRs
accordingly:

- `fix: ...` — patch release
- `feat: ...` — minor release
- `feat!: ...` or a `BREAKING CHANGE:` footer — major release
- `chore: ...`, `docs: ...`, `ci: ...`, `test: ...`, `refactor: ...`,
  `build: ...`, `perf: ...`, `revert: ...` — recorded but do not
  trigger a release on their own

The **PR title** job in
[`.github/workflows/pr-title.yml`](../.github/workflows/pr-title.yml)
enforces this on every PR (checked on open, edit, and each push), so a
non-conforming title fails CI instead of silently confusing
release-plz after a squash-merge.

## One-time setup

These steps are performed once by a maintainer with crates.io and
repository admin access.

1. **Allow the release PR job to open pull requests.** In repo
   Settings → Actions → General → Workflow permissions, enable "Allow
   GitHub Actions to create and approve pull requests." Without this,
   the default `GITHUB_TOKEN` can't open the release PR.
2. **Publish the first release manually.** crates.io Trusted
   Publishing can't be configured for a crate that has never been
   published, so the very first release has to go up by hand:
   ```console
   cargo login
   cargo publish
   ```
   Use a crates.io API token scoped to this crate for `cargo login`.
3. **Register the workflow as a Trusted Publisher.** On the crate's
   crates.io settings page, add a GitHub Actions trusted publisher
   with:
   - Repository owner: `slaptijack`
   - Repository name: `human-exception`
   - Workflow filename: `release-plz.yml`
   - Environment: (leave blank)
4. **Revoke the temporary token** used in step 2. From here on, the
   `release-plz-release` job authenticates to crates.io via OIDC
   (`id-token: write`) — no `CARGO_REGISTRY_TOKEN` secret is stored in
   the repository.

## Validating changes safely

To check that the pipeline is configured correctly without publishing
anything, run the workflow manually with its dry-run input:

```console
gh workflow run release-plz.yml
```

(or trigger it from the Actions tab). `workflow_dispatch` runs only
the release job, with `dry_run` defaulting to `true`, which runs
`release-plz release --dry-run` and reports what it would publish,
tag, and release without doing any of it. Manually dispatching with
the `dry_run` input set to `false` performs a real release outside the
normal push-triggered flow, if ever needed.

## Recovering from a failed release

If the `release-plz-release` job fails partway (for example, crates.io
is temporarily unavailable), it's safe to simply re-run the failed
workflow run from the Actions tab. release-plz checks crates.io before
publishing each package, so it will skip anything already published
and only retry what's missing — it will not create a duplicate tag,
GitHub release, or crates.io publish.

## If `Cargo.toml`'s version sits ahead of the published crate for a while

release-plz expects the release PR it opens to be merged (which
publishes that version) before more commits land. If `Cargo.toml`'s
version instead stays ahead of what's on crates.io for an extended
period — as happened between #14 and #37, when a release-job bug meant
`0.1.1` never actually got tagged or published while several more
PRs merged — release-plz's changelog updater can stop picking up new
commits into that pending version's changelog entry. It doesn't
retroactively catch up once the version finally publishes: the
`CHANGELOG.md` entry and GitHub release notes for that release may
end up missing entries for commits that landed while it sat pending
(see #38, where 8 of the 17 commits that shipped in `v0.1.1` were
missing from both).

If this happens, there's no automated recovery — release-plz doesn't
rewrite a changelog entry for a version it has already tagged. Fix
`CHANGELOG.md` by hand (compare `git log <previous-tag>..<tag>` against
the changelog entry) and edit the GitHub release notes to match
(`gh release edit <tag> --notes-file -`). The safest way to avoid this
altogether is to merge the release PR promptly instead of letting
`Cargo.toml`'s version drift ahead of the registry for long.
