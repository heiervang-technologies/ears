#!/usr/bin/env bash
#
# Common test utilities and setup for BATS tests
#

# Setup test environment
setup_test_env() {
    # Create temporary test directory
    export TEST_TEMP_DIR="$(mktemp -d)"
    export XDG_RUNTIME_DIR="$TEST_TEMP_DIR/runtime"
    export HOME="$TEST_TEMP_DIR/home"

    mkdir -p "$XDG_RUNTIME_DIR"
    mkdir -p "$HOME/.config/ears"
    mkdir -p "$HOME/.local/share/ears-sounds"

    # Add mocks to PATH
    export ORIGINAL_PATH="$PATH"
    export PATH="$BATS_TEST_DIRNAME/../mocks:$PATH"

    # Mock environment variables
    export WAYLAND_DISPLAY="wayland-0"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
}

# Cleanup test environment
teardown_test_env() {
    if [[ -n "${TEST_TEMP_DIR:-}" ]] && [[ -d "$TEST_TEMP_DIR" ]]; then
        rm -rf "$TEST_TEMP_DIR"
    fi
    export PATH="$ORIGINAL_PATH"
}

# Source the ears script functions for testing
# This extracts functions without executing the main script
source_ears_functions() {
    # We'll need to source specific functions or mock the entire script
    # For now, we'll test the script by executing it with arguments
    export EARS_BIN="$BATS_TEST_DIRNAME/../../bin/ears"
}

# Mock command tracker - records calls to mock commands
init_mock_tracker() {
    export MOCK_CALL_LOG="$TEST_TEMP_DIR/mock_calls.log"
    : > "$MOCK_CALL_LOG"
}

record_mock_call() {
    local cmd="$1"
    shift
    echo "$cmd $*" >> "$MOCK_CALL_LOG"
}

get_mock_calls() {
    local cmd="$1"
    grep "^${cmd} " "$MOCK_CALL_LOG" 2>/dev/null || true
}

count_mock_calls() {
    local cmd="$1"
    get_mock_calls "$cmd" | wc -l
}

# Assert helpers
assert_success() {
    [[ "$status" -eq 0 ]] || {
        echo "Expected success (exit code 0) but got: $status" >&2
        return 1
    }
}

assert_failure() {
    [[ "$status" -ne 0 ]] || {
        echo "Expected failure (non-zero exit code) but got: $status" >&2
        return 1
    }
}

assert_file_exists() {
    [[ -f "$1" ]] || {
        echo "Expected file to exist: $1" >&2
        return 1
    }
}

assert_file_not_exists() {
    [[ ! -f "$1" ]] || {
        echo "Expected file to not exist: $1" >&2
        return 1
    }
}

assert_file_contains() {
    local file="$1"
    local pattern="$2"
    assert_file_exists "$file"
    grep -q "$pattern" "$file" || {
        echo "Expected file $file to contain: $pattern" >&2
        echo "Actual contents:" >&2
        cat "$file" >&2
        return 1
    }
}

assert_output_contains() {
    local pattern="$1"
    echo "$output" | grep -q "$pattern" || {
        echo "Expected output to contain: $pattern" >&2
        echo "Actual output: $output" >&2
        return 1
    }
}

# Create a mock PID file with a running process
create_mock_recording() {
    local state_dir="$XDG_RUNTIME_DIR/ears"
    mkdir -p "$state_dir"

    # Start a sleep process that we can control
    sleep 60 &
    local pid=$!
    echo "$pid" > "$state_dir/recording.pid"
    echo "$pid"
}

kill_mock_recording() {
    local pid="$1"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}
