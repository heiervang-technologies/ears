# Quick Start Guide for Testing ears

## Install BATS

```bash
# Ubuntu/Debian
sudo apt install bats

# Or from source
git clone https://github.com/bats-core/bats-core.git
cd bats-core
sudo ./install.sh /usr/local
```

## Run All Tests

```bash
cd ears
./tests/run_tests.sh
```

## Common Commands

```bash
# Run only unit tests
./tests/run_tests.sh --unit-only

# Run only integration tests
./tests/run_tests.sh --integration-only

# Run with verbose output
./tests/run_tests.sh --verbose

# Run tests matching a pattern
./tests/run_tests.sh --filter config

# Run a specific test file
bats tests/unit/test_config.bats

# Get help
./tests/run_tests.sh --help
```

## What Gets Tested

- ✅ Configuration (server URL, device selection)
- ✅ Device listing and filtering
- ✅ State management (locks, PIDs, cleanup)
- ✅ Recording workflow (start/stop)
- ✅ Transcription pipeline
- ✅ Error handling
- ✅ Notifications and audio feedback

## Test Structure

```
tests/
├── unit/          # Test individual features
├── integration/   # Test complete workflows
├── mocks/         # Mock external commands
└── fixtures/      # Sample data
```

## Example Test Output

```
ears Test Suite
===============

Running unit tests...

  ✓ test_config.bats
  ✓ test_device_listing.bats
  ✓ test_state_management.bats

Running integration tests...

  ✓ test_workflow.bats

===============
Test Summary
===============
Total test files: 4
Passed: 4
Failed: 0

All tests passed!
```

## Adding New Tests

1. Create a `.bats` file in `tests/unit/` or `tests/integration/`
2. Use the template:

```bash
#!/usr/bin/env bats

load ../test_helper

setup() {
    setup_test_env
    init_mock_tracker
}

teardown() {
    teardown_test_env
}

@test "description of what you're testing" {
    run "$EARS_BIN" --some-command
    assert_success
    assert_output_contains "expected output"
}
```

3. Run your new test:
```bash
bats tests/unit/your_new_test.bats
```

## Troubleshooting

**Tests won't run**
- Make sure BATS is installed: `which bats`
- Make test runner executable: `chmod +x tests/run_tests.sh`

**Mock commands not found**
- Make mocks executable: `chmod +x tests/mocks/*`

**Tests hang or timeout**
- Run with `--verbose` to see which test hangs
- Check if background processes are cleaned up

For more details, see [tests/README.md](README.md)
