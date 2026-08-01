# Releasing ears

`Cargo.toml` is the single source of truth for the ears version. Releases use
[Semantic Versioning](https://semver.org/) and are created from intentional
version bumps; commit counts or generated build numbers are not versions.

## Release process

1. Choose the next Semantic Version based on the user-visible changes since the
   previous release.
2. Update the package version in `Cargo.toml`.
3. Run `cargo check` to update the root package entry in `Cargo.lock`.
4. Move the relevant entries from `Unreleased` into a dated version section in
   `CHANGELOG.md`.
5. Run `./scripts/check-version.sh`, the test suite, and Clippy before merging.

After the bump reaches `main`, the release workflow builds the binary, verifies
that `ears --version` agrees with the package version, creates the corresponding
`v{version}` tag, and publishes a release with the same name. Pushes that do not
change the package version are skipped because that tag already exists.

Historical tags are immutable. If an old release used a generated version or
otherwise disagreed with its package metadata, advance to a new valid Semantic
Version instead of moving or deleting the published tag.
