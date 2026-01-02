# ears Test Suite

Comprehensive test suite for the ears speech recognition daemon.

## Overview

This test suite uses [BATS (Bash Automated Testing System)](https://github.com/bats-core/bats-core) to test the ears bash script. It includes:

- **Unit tests** - Test individual functions and features in isolation
- **Integration tests** - Test the full recording and transcription workflow
- **Mock system** - Mock external dependencies (PipeWire, curl, ydotool, etc.)
- **Test fixtures** - Sample data for consistent test scenarios

## Installation

### Install BATS

#### Ubuntu/Debian
```bash
sudo apt install bats
```

#### From source
```bash
git clone https://github.com/bats-core/bats-core.git
cd bats-core
sudo ./install.sh /usr/local
```

## Running Tests

### Run all tests
```bash
./tests/run_tests.sh
```

### Run only unit tests
```bash
./tests/run_tests.sh --unit-only
```

### Run only integration tests
```bash
./tests/run_tests.sh --integration-only
```

### Run with verbose output
```bash
./tests/run_tests.sh --verbose
```

### Run specific test files
```bash
./tests/run_tests.sh --filter config
```

### Run individual BATS files directly
```bash
bats tests/unit/test_config.bats
bats tests/integration/test_workflow.bats
```

## Test Structure

```
tests/
├── run_tests.sh              # Test runner script
├── test_helper.bash          # Common test utilities
├── unit/                     # Unit tests
│   ├── test_config.bats          # Configuration management tests
│   ├── test_device_listing.bats  # Device listing tests
│   └── test_state_management.bats # State file management tests
├── integration/              # Integration tests
│   └── test_workflow.bats        # Full workflow tests
├── mocks/                    # Mock external commands
│   ├── pw-cli                    # Mock PipeWire CLI
│   ├── pw-record                 # Mock PipeWire recorder
│   ├── curl                      # Mock HTTP client
│   ├── ydotool                   # Mock keyboard input tool
│   ├── notify-send               # Mock notification daemon
│   ├── paplay                    # Mock audio player
│   ├── fzf                       # Mock fuzzy finder
│   └── jq                        # Mock JSON processor
└── fixtures/                 # Test data
    ├── pw-cli-output.txt         # Sample PipeWire device list
    ├── whisper-response.json     # Sample transcription response
    ├── whisper-response-empty.json   # Empty transcription
    └── whisper-response-silence.json # Silence artifact
```

## Test Categories

### Unit Tests

#### Configuration Tests (`test_config.bats`)
- Default server configuration
- Setting custom server URLs
- Saving and loading device configuration
- Help text display

#### Device Listing Tests (`test_device_listing.bats`)
- Listing audio input devices
- Filtering out video sources and audio sinks
- Device selection with fzf
- Showing current device configuration

#### State Management Tests (`test_state_management.bats`)
- State directory creation
- Lock file handling
- PID file management
- Stale process cleanup
- Debug logging

### Integration Tests

#### Workflow Tests (`test_workflow.bats`)
- Starting and stopping recordings
- Transcription pipeline
- Audio feedback (beeps)
- Error handling (server down, empty audio, etc.)
- Silence artifact filtering
- Recording timeout

## Mock System

The test suite mocks external dependencies to run tests without requiring:
- PipeWire audio system
- whisper.cpp server
- ydotool daemon
- Desktop notification system

Mocks are implemented as executable scripts in `tests/mocks/` that are added to PATH during test execution.

### Mock Behavior

- **pw-cli**: Returns sample device list from fixtures
- **pw-record**: Creates minimal WAV files
- **curl**: Returns fixture responses for health checks and transcription
- **ydotool**: Logs typed text to a file for verification
- **notify-send**: Logs notifications for verification
- **paplay**: Silent (no actual audio playback)
- **fzf**: Returns configurable selections via environment variables

### Controlling Mock Behavior

Use environment variables to control mock responses:

```bash
# Make fzf return a specific selection
export MOCK_FZF_SELECTION="device_name	Device Description"

# Make fzf simulate cancellation
export MOCK_FZF_CANCEL=1
```

## Test Helpers

The `test_helper.bash` file provides common utilities:

### Environment Setup
```bash
setup_test_env      # Creates isolated test environment
teardown_test_env   # Cleans up after tests
```

### Mock Tracking
```bash
init_mock_tracker              # Initialize call logging
record_mock_call "cmd" "args"  # Record a mock call
get_mock_calls "cmd"           # Get all calls to a command
count_mock_calls "cmd"         # Count calls to a command
```

### Assertions
```bash
assert_file_exists "path"              # Assert file exists
assert_file_not_exists "path"          # Assert file doesn't exist
assert_file_contains "path" "pattern"  # Assert file contains pattern
assert_output_contains "pattern"       # Assert command output contains pattern
```

### Mock Recording Helpers
```bash
create_mock_recording  # Creates a mock recording with PID file
kill_mock_recording    # Cleans up mock recording
```

## Writing New Tests

### Basic Test Structure

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

@test "description of test" {
    # Arrange
    echo "test-value" > "$HOME/.config/ears/some-config"

    # Act
    run "$EARS_BIN" --some-command

    # Assert
    assert_success
    assert_output_contains "expected output"
}
```

### Testing the Main Script

The `$EARS_BIN` variable points to `bin/ears`:

```bash
@test "example test" {
    run "$EARS_BIN" --help
    assert_success
    assert_output_contains "Usage"
}
```

### Testing with Mock Commands

```bash
@test "curl is called correctly" {
    run "$EARS_BIN" --some-command
    assert_success

    # Verify curl was called
    [[ $(count_mock_calls "curl") -eq 1 ]]
}
```

## Continuous Integration

To run tests in CI:

```yaml
# .github/workflows/test.yml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install BATS
        run: sudo apt-get install -y bats
      - name: Run tests
        run: ./tests/run_tests.sh
```

## Debugging Tests

### Run tests with verbose output
```bash
./tests/run_tests.sh --verbose
```

### Run single test file
```bash
bats tests/unit/test_config.bats
```

### Run specific test by name
```bash
bats tests/unit/test_config.bats --filter "default whisper server"
```

### Check mock call logs
Mocks record their calls in `$TEST_TEMP_DIR/mock_calls.log`:
```bash
@test "debug example" {
    run "$EARS_BIN" --some-command

    # Print what was called
    cat "$MOCK_CALL_LOG"
}
```

### Inspect temporary files
Tests create isolated environments in `$TEST_TEMP_DIR`:
```bash
@test "debug example" {
    run "$EARS_BIN" --some-command

    # Print temporary directory location
    echo "Test dir: $TEST_TEMP_DIR"

    # Inspect files
    ls -la "$TEST_TEMP_DIR"
}
```

## Coverage

The test suite covers:

- ✅ Configuration management (server, device)
- ✅ Device listing and selection
- ✅ State management (locks, PIDs, cleanup)
- ✅ Recording start/stop
- ✅ Transcription pipeline
- ✅ Error handling
- ✅ Audio feedback
- ✅ Notification system
- ✅ Silence detection and filtering

## Known Limitations

- Tests don't verify actual audio quality or transcription accuracy
- Tests don't test real PipeWire integration (uses mocks)
- Tests don't test actual keyboard input (ydotool is mocked)
- Some timing-dependent tests may be flaky on slow systems

## Contributing

When adding new features to ears:

1. Write unit tests for individual functions
2. Write integration tests for user-facing workflows
3. Update test fixtures if needed
4. Run the full test suite before submitting PRs

## Troubleshooting

### Tests fail with "bats: command not found"
Install BATS using the instructions in the Installation section.

### Tests fail with permission errors
Ensure mock scripts are executable:
```bash
chmod +x tests/mocks/*
```

### Tests timeout or hang
Some tests use `timeout` to prevent hanging. If tests are slow:
- Run with `--verbose` to see which test is slow
- Increase timeout values in the test file
- Check if background processes are being cleaned up properly

### Mock commands not found
Ensure mocks are in PATH by checking test setup:
```bash
echo $PATH  # Should include tests/mocks
```
