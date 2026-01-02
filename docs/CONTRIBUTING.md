# Contributing to ears

Thank you for your interest in contributing to ears! This guide will help you get started with development, testing, and submitting contributions.

## Table of Contents

- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Release Process](#release-process)
- [Getting Help](#getting-help)

## Development Setup

### Prerequisites

Before you start, ensure you have:

1. **Rust toolchain** (stable, latest version recommended)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source $HOME/.cargo/env
   ```

2. **System dependencies** (see [INSTALL.md](INSTALL.md) for distro-specific commands)
   - PipeWire
   - ydotool
   - notify-send
   - paplay
   - fzf
   - jq

3. **Whisper.cpp server** running locally
   ```bash
   git clone https://github.com/ggerganov/whisper.cpp
   cd whisper.cpp
   make server
   bash ./models/download-ggml-model.sh base.en
   ./server -m models/ggml-base.en.bin -p 8178 &
   ```

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/heiervang-technologies/ears
cd ears

# Build the project
cargo build

# Run tests
cargo test

# Build release version
cargo build --release

# Install locally for testing
cargo install --path .
```

### Recommended Tools

- **rust-analyzer** - LSP for IDE integration
- **cargo-watch** - Auto-rebuild on file changes
  ```bash
  cargo install cargo-watch
  cargo watch -x test
  ```
- **cargo-edit** - Manage dependencies from CLI
  ```bash
  cargo install cargo-edit
  ```

## Project Structure

```
ears/
├── src/               # Rust source code
│   ├── main.rs        # Binary entry point
│   ├── lib.rs         # Library root
│   ├── cli.rs         # CLI argument parsing
│   ├── config.rs      # Configuration management
│   ├── lock.rs        # File locking
│   ├── state.rs       # State management
│   ├── process.rs     # Process control
│   ├── audio.rs       # Audio device management
│   ├── recording.rs   # Recording logic
│   ├── whisper.rs     # Whisper.cpp client
│   ├── desktop.rs     # Desktop integration
│   └── tui/           # TUI components
│       ├── mod.rs
│       ├── app.rs
│       ├── ui.rs
│       └── event.rs
├── tests/             # Integration tests
│   ├── config.rs
│   ├── state.rs
│   ├── whisper_integration.rs
│   └── tui.rs
├── docs/              # Documentation
│   ├── INSTALL.md     # User installation guide
│   ├── ARCHITECTURE.md # Technical architecture
│   └── CONTRIBUTING.md # This file
├── bin/               # Legacy Bash version
│   └── ears
├── Cargo.toml         # Rust dependencies
└── README.md          # Project overview
```

### Module Responsibilities

| Module | Iteration | Purpose |
|--------|-----------|---------|
| `config` | 1 | Configuration loading, saving, validation |
| `lock` | 2 | File-based locking to prevent concurrent instances |
| `state` | 2 | Track recording state (Idle/Recording) |
| `process` | 2 | Spawn and manage child processes (pw-record) |
| `audio` | 3 | Audio device enumeration and selection |
| `recording` | 3 | High-level recording orchestration |
| `whisper` | 4 | HTTP client for whisper.cpp API |
| `desktop` | 6 | Notifications, audio feedback, text input |
| `tui` | 7 | Terminal user interface |
| `cli` | 1 | Command-line argument parsing |

## Coding Standards

### Rust Style

We follow the official [Rust Style Guide](https://doc.rust-lang.org/1.0.0/style/):

- **Use `rustfmt`** for formatting:
  ```bash
  cargo fmt
  ```

- **Use `clippy`** for linting:
  ```bash
  cargo clippy -- -D warnings
  ```

- **Follow naming conventions**:
  - Types: `PascalCase` (e.g., `StateManager`)
  - Functions: `snake_case` (e.g., `acquire_lock`)
  - Constants: `SCREAMING_SNAKE_CASE` (e.g., `MAX_RETRIES`)
  - Modules: `snake_case` (e.g., `whisper`)

### Documentation

- **Every public item** must have doc comments:
  ```rust
  /// Acquires an exclusive lock on the specified file.
  ///
  /// # Arguments
  ///
  /// * `path` - Path to the lock file
  ///
  /// # Returns
  ///
  /// * `Ok(FileLock)` - Lock successfully acquired
  /// * `Err(LockError)` - Lock already held or I/O error
  ///
  /// # Example
  ///
  /// ```no_run
  /// use ears::FileLock;
  /// use std::path::Path;
  ///
  /// let lock = FileLock::acquire(Path::new("/tmp/my.lock"))?;
  /// // Lock automatically released when `lock` goes out of scope
  /// # Ok::<(), ears::LockError>(())
  /// ```
  pub fn acquire(path: &Path) -> Result<Self, LockError> {
      // ...
  }
  ```

- **Module-level docs** explain purpose and usage:
  ```rust
  //! File locking implementation using POSIX flock.
  //!
  //! This module provides a simple RAII-style file lock that automatically
  //! releases on drop. Locks are advisory (cooperative) and use the flock(2)
  //! system call.
  ```

- **Run doc tests**:
  ```bash
  cargo test --doc
  ```

### Error Handling

- **Use `Result<T, E>` for fallible operations**
- **Use `anyhow::Result` for application errors** (main.rs)
- **Use custom error types for library errors** (lib.rs modules)
  ```rust
  use thiserror::Error;

  #[derive(Error, Debug)]
  pub enum ConfigError {
      #[error("Invalid server URL: {0}")]
      InvalidUrl(#[from] url::ParseError),

      #[error("Config file not found: {0}")]
      NotFound(PathBuf),

      #[error("I/O error: {0}")]
      Io(#[from] std::io::Error),
  }
  ```

- **Always provide context** when propagating errors:
  ```rust
  fs::read_to_string(&path)
      .with_context(|| format!("Failed to read config file: {}", path.display()))?
  ```

### Testing

- **Write unit tests** for all public functions
- **Write integration tests** for workflows
- **Use meaningful test names**:
  ```rust
  #[test]
  fn test_config_loads_from_file() { }

  #[test]
  fn test_config_validates_invalid_url() { }
  ```

- **Use test fixtures** for temporary files:
  ```rust
  use tempfile::TempDir;

  #[test]
  fn test_with_temp_dir() {
      let temp_dir = TempDir::new().unwrap();
      let config_path = temp_dir.path().join("config.toml");
      // Test code using config_path
      // temp_dir automatically cleaned up on drop
  }
  ```

- **Mock external dependencies**:
  ```rust
  use wiremock::{MockServer, Mock, ResponseTemplate};
  use wiremock::matchers::{method, path};

  #[tokio::test]
  async fn test_whisper_client_success() {
      let mock_server = MockServer::start().await;

      Mock::given(method("POST"))
          .and(path("/inference"))
          .respond_with(ResponseTemplate::new(200)
              .set_body_json(json!({"text": "hello world"})))
          .mount(&mock_server)
          .await;

      let client = WhisperClient::new(mock_server.uri());
      let result = client.transcribe(audio_path).await;

      assert_eq!(result.unwrap(), "hello world");
  }
  ```

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test config

# Run a specific test
cargo test test_config_loads_from_file

# Run with output
cargo test -- --nocapture

# Run tests in parallel (default)
cargo test

# Run tests serially (for tests that can't run in parallel)
cargo test -- --test-threads=1
```

### Test Organization

Tests are split into two categories:

1. **Unit tests** (in same file as code):
   ```rust
   // src/config.rs

   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn test_config_default() {
           let config = Config::default();
           assert_eq!(config.whisper_server.as_str(), "http://localhost:8178");
       }
   }
   ```

2. **Integration tests** (in `tests/` directory):
   ```rust
   // tests/whisper_integration.rs

   use ears::WhisperClient;

   #[tokio::test]
   async fn test_whisper_end_to_end() {
       // Test using public API only
   }
   ```

### Test Coverage

We aim for **80%+ code coverage**. Check coverage locally with `tarpaulin`:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html
```

Open `tarpaulin-report.html` in a browser to see coverage report.

### Testing Best Practices

1. **Test behavior, not implementation**
2. **Use descriptive assertions**:
   ```rust
   assert_eq!(result, expected, "Transcription should filter 'Thank you.' artifact");
   ```
3. **Test edge cases**:
   - Empty inputs
   - Very large inputs
   - Invalid inputs
   - Concurrent access
4. **Clean up test resources** (use RAII, tempfile, etc.)
5. **Avoid timing-dependent tests** (use mocks, not sleeps)

## Submitting Changes

### Workflow

1. **Create an issue** (optional but recommended for large changes)
   - Describe the problem or feature
   - Discuss approach before implementing

2. **Fork the repository** and create a branch:
   ```bash
   git checkout -b feature/my-awesome-feature
   ```

3. **Make your changes**:
   - Write code
   - Add tests
   - Update documentation
   - Run `cargo fmt` and `cargo clippy`

4. **Commit your changes**:
   ```bash
   git add .
   git commit -m "feat: add support for multi-language detection"
   ```

5. **Push to your fork**:
   ```bash
   git push origin feature/my-awesome-feature
   ```

6. **Open a pull request**:
   - Describe what changed and why
   - Link to related issues
   - Ensure CI passes

### Commit Message Format

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types**:
- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation changes
- `test:` - Adding or updating tests
- `refactor:` - Code refactoring (no behavior change)
- `perf:` - Performance improvement
- `chore:` - Maintenance (dependencies, tooling, etc.)
- `ci:` - CI/CD changes

**Examples**:
```
feat(whisper): add retry logic with exponential backoff

Implements automatic retry for whisper.cpp API calls with
exponential backoff (50ms initial, 500ms max, 3 retries).

Closes #42
```

```
fix(state): prevent race condition in PID file cleanup

The state manager was not atomically checking PID existence
before removing the PID file, causing rare crashes when
multiple instances started simultaneously.
```

```
docs(install): add troubleshooting section for ydotool

Users frequently encounter permission issues with ydotool.
Added a troubleshooting section covering:
- Checking daemon status
- Socket permissions
- Input group membership
```

### Pull Request Checklist

Before submitting a PR, ensure:

- [ ] Code follows Rust style guide (`cargo fmt`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`)
- [ ] All tests pass (`cargo test`)
- [ ] New code has tests (unit or integration)
- [ ] Public APIs have doc comments
- [ ] Documentation updated (README, docs/, etc.)
- [ ] Commit messages follow Conventional Commits
- [ ] CI checks pass (formatting, clippy, tests, build)

### Code Review Process

1. **Automated checks** run on GitHub Actions (formatting, clippy, tests)
2. **Maintainer review** (usually within 48 hours)
3. **Feedback and iteration** (address comments)
4. **Approval and merge** (squash and merge by default)

## Release Process

(For maintainers)

### Version Numbering

We follow [Semantic Versioning](https://semver.org/):
- `MAJOR.MINOR.PATCH`
- Increment `MAJOR` for breaking changes
- Increment `MINOR` for new features (backwards-compatible)
- Increment `PATCH` for bug fixes

### Release Steps

1. **Update version in `Cargo.toml`**:
   ```toml
   [package]
   version = "0.2.0"
   ```

2. **Update CHANGELOG.md**:
   ```markdown
   ## [0.2.0] - 2026-01-15

   ### Added
   - Multi-language detection
   - TUI mode for interactive monitoring

   ### Fixed
   - Race condition in state management
   - Memory leak in whisper client
   ```

3. **Commit and tag**:
   ```bash
   git commit -am "chore: release v0.2.0"
   git tag -a v0.2.0 -m "Release v0.2.0"
   git push origin main --tags
   ```

4. **Publish to crates.io**:
   ```bash
   cargo publish
   ```

5. **Create GitHub release**:
   - Go to [Releases](https://github.com/heiervang-technologies/ears/releases)
   - Click "Draft a new release"
   - Select tag `v0.2.0`
   - Copy CHANGELOG content
   - Attach binary artifacts (optional)
   - Publish release

## Development Guidelines

### Adding a New Feature

1. **Read existing code** to understand patterns
2. **Create an issue** to discuss the feature
3. **Design the API** (public functions, types)
4. **Implement incrementally**:
   - Core functionality
   - Error handling
   - Tests
   - Documentation
5. **Test thoroughly** (unit, integration, manual)
6. **Update docs** (README, docs/, code comments)
7. **Submit PR** with clear description

### Fixing a Bug

1. **Reproduce the bug** (create a test case)
2. **Identify root cause** (use debugger, logs, etc.)
3. **Fix the bug** (minimal change, don't refactor)
4. **Add regression test** (ensure bug doesn't return)
5. **Verify fix** (run all tests, manual testing)
6. **Submit PR** with clear description and test case

### Refactoring

1. **Ensure tests pass** before refactoring
2. **Refactor incrementally** (small, safe changes)
3. **Keep tests passing** at each step
4. **Don't change behavior** (refactoring should be transparent)
5. **Update docs** if API changed

### Performance Optimization

1. **Profile first** (use `cargo flamegraph`, `perf`, etc.)
   ```bash
   cargo install flamegraph
   cargo flamegraph --bin ears
   ```
2. **Identify bottleneck** (don't optimize blindly)
3. **Benchmark before and after**:
   ```rust
   #[bench]
   fn bench_transcribe(b: &mut Bencher) {
       b.iter(|| {
           // Code to benchmark
       });
   }
   ```
4. **Document performance assumptions** (e.g., "optimized for files < 10MB")

## Getting Help

### Resources

- **README.md** - Project overview and quick start
- **docs/INSTALL.md** - Detailed installation and usage
- **docs/ARCHITECTURE.md** - Technical architecture
- **docs/CONTRIBUTING.md** - This file
- **API docs** - Run `cargo doc --open`

### Communication

- **GitHub Issues** - Bug reports, feature requests
  - [https://github.com/heiervang-technologies/ears/issues](https://github.com/heiervang-technologies/ears/issues)
- **GitHub Discussions** - General questions, ideas
  - [https://github.com/heiervang-technologies/ears/discussions](https://github.com/heiervang-technologies/ears/discussions)
- **Pull Requests** - Code contributions
  - [https://github.com/heiervang-technologies/ears/pulls](https://github.com/heiervang-technologies/ears/pulls)

### Common Questions

**Q: How do I test my changes without installing?**
```bash
cargo run -- --help
cargo run -- --list
# etc.
```

**Q: How do I debug a specific test?**
```bash
# Add println!() or dbg!() in test code
cargo test test_name -- --nocapture
```

**Q: How do I see what syscalls are being made?**
```bash
strace -f target/debug/ears 2>&1 | less
```

**Q: How do I test against a different whisper.cpp model?**
```bash
# Start whisper server with different model
./whisper.cpp/server -m models/ggml-small.en.bin -p 8178

# Configure ears
ears --server http://localhost:8178
```

**Q: Can I contribute without writing Rust code?**

Yes! Contributions welcome for:
- Documentation improvements
- Bug reports with detailed reproduction steps
- Feature suggestions with use cases
- Testing on different distros/hardware
- Bash script improvements (legacy version)

## Code of Conduct

Be respectful and constructive. We follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

---

Thank you for contributing to ears! 🎉
