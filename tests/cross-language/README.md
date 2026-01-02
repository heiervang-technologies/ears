# Cross-Language Test Suite

This directory contains tests that work across both Bash and Rust implementations of `ears`.

## Overview

The cross-language test suite ensures behavioral parity between the legacy Bash implementation and the new Rust implementation. Tests are defined in JSON format and can be executed against either implementation.

## Test Format

Tests are defined in JSON files with the following structure:

```json
{
  "name": "Test name",
  "description": "What this test validates",
  "setup": {
    "env": {
      "HOME": "/tmp/test-home",
      "XDG_RUNTIME_DIR": "/tmp/test-runtime"
    },
    "files": {
      "/path/to/file": "content"
    }
  },
  "command": {
    "args": ["--server"],
    "stdin": null
  },
  "assertions": {
    "exit_code": 0,
    "stdout_contains": ["Current server"],
    "stdout_not_contains": [],
    "stderr_contains": [],
    "files_exist": [],
    "files_not_exist": [],
    "file_contains": {
      "/path/to/file": "expected content"
    }
  }
}
```

## Running Tests

### Against Bash Implementation

```bash
python3 tests/cross-language/runner.py --impl bash
```

### Against Rust Implementation

```bash
python3 tests/cross-language/runner.py --impl rust
```

### Against Both

```bash
python3 tests/cross-language/runner.py --impl both
```

## Test Categories

- `config/` - Configuration management tests
- `state/` - State management tests
- `device/` - Device listing and selection tests
- `integration/` - End-to-end workflow tests
