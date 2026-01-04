# Releasing

This project uses [release-plz](https://release-plz.dev/) for automated releases.

## How It Works

Release-plz automates the entire release process:

1. **Automatic Version Bumping**: On every push to `main`, release-plz analyzes commits and API changes to determine the next version according to [Semantic Versioning](https://semver.org/)
2. **Changelog Generation**: Creates and maintains a CHANGELOG.md based on [conventional commits](https://www.conventionalcommits.org/)
3. **Release PR**: Opens a pull request with version bumps and changelog updates
4. **Publishing**: When the release PR is merged, automatically publishes to crates.io and creates a GitHub release

## Commit Message Format

To get the most out of automatic changelog generation, use conventional commit messages:

- `feat: add new feature` - New features (minor version bump)
- `fix: resolve bug` - Bug fixes (patch version bump)
- `perf: improve performance` - Performance improvements
- `refactor: restructure code` - Code refactoring
- `docs: update documentation` - Documentation changes
- `build: update dependencies` - Build system changes
- `ci: update workflows` - CI/CD changes
- `test: add tests` - Test additions/updates
- `chore: routine tasks` - Routine tasks (not included in changelog)

For breaking changes, add `!` after the type or add `BREAKING CHANGE:` in the commit body:
```
feat!: change API signature

BREAKING CHANGE: The `process` method now requires an additional parameter
```

## Release Process

1. **Make changes** on a feature branch following conventional commits
2. **Merge to main** - CI will run tests and checks
3. **Wait for release-plz** - A release PR will be created automatically
4. **Review the release PR** - Check the version bump and changelog
5. **Merge the release PR** - The package will be published automatically

## Setup Requirements

### For Maintainers

To enable releases, the following secrets must be configured in the repository:

1. **CARGO_REGISTRY_TOKEN**: Create a token at https://crates.io/me
   - Required scopes: `publish-new` and `publish-update`
   - Add to repository secrets

2. **GitHub Actions Permissions**:
   - Go to Settings → Actions → General
   - Under "Workflow permissions", enable "Allow GitHub Actions to create and approve pull requests"

## Manual Release

If you need to release manually:

```bash
# Install release-plz
cargo install release-plz

# Create a release PR
release-plz release-pr

# Or publish directly
release-plz release
```

## Configuration

Release behavior is configured in `release-plz.toml`. See the [release-plz documentation](https://release-plz.dev/docs/config) for available options.
