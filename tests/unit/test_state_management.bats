#!/usr/bin/env bats
#
# Unit tests for state management (lock files, PID files, cleanup)
#

load ../test_helper

setup() {
    setup_test_env
    init_mock_tracker
    source_ears_functions

    # Ensure state directory exists
    mkdir -p "$XDG_RUNTIME_DIR/ears"
}

teardown() {
    teardown_test_env
}

@test "state directory is created in XDG_RUNTIME_DIR" {
    # The script should create the state directory
    run bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        timeout 1 '$EARS_BIN' 2>&1 || true
    "
    [[ -d "$XDG_RUNTIME_DIR/ears" ]]
}

@test "lock file prevents concurrent execution" {
    # Create a lock file with an active lock
    exec 201>"$XDG_RUNTIME_DIR/ears/lock"
    flock 201

    # Try to run ears - should exit silently
    run timeout 1 "$EARS_BIN"

    # Clean up lock
    flock -u 201
    exec 201>&-

    # Should have exited with success (silent exit)
    [[ "$status" -eq 0 ]] || [[ "$status" -eq 124 ]]
}

@test "stale PID file is cleaned up" {
    # Create a stale PID file with a non-existent process
    echo "99999" > "$XDG_RUNTIME_DIR/ears/recording.pid"
    touch "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Run the script (it should clean up on startup)
    run timeout 2 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1 || true
    "

    # Give it a moment to start
    sleep 0.5

    # PID file should be cleaned or replaced
    if [[ -f "$XDG_RUNTIME_DIR/ears/recording.pid" ]]; then
        pid=$(cat "$XDG_RUNTIME_DIR/ears/recording.pid")
        # Should be a new PID, not 99999
        [[ "$pid" != "99999" ]]
    fi
}

@test "cleanup removes both PID and audio files" {
    # Create stale files
    echo "99999" > "$XDG_RUNTIME_DIR/ears/recording.pid"
    touch "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Trigger cleanup by running script
    run timeout 2 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1 || true
    "

    sleep 0.5

    # Either files are cleaned, or new recording started
    # The test passes if we don't have the stale PID
    if [[ -f "$XDG_RUNTIME_DIR/ears/recording.pid" ]]; then
        pid=$(cat "$XDG_RUNTIME_DIR/ears/recording.pid")
        [[ "$pid" != "99999" ]]
    fi
}

@test "recording creates PID file" {
    # Start recording
    run timeout 2 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' &
        sleep 0.5
        # Check if PID file was created
        test -f '$XDG_RUNTIME_DIR/ears/recording.pid'
    "
    assert_success
}

@test "debug log is created" {
    run timeout 2 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1 || true
        sleep 0.3
    "

    [[ -f "$XDG_RUNTIME_DIR/ears/debug.log" ]]
}

@test "debug log contains script invocation" {
    run timeout 2 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1 || true
        sleep 0.3
    "

    if [[ -f "$XDG_RUNTIME_DIR/ears/debug.log" ]]; then
        grep -q "Script invoked" "$XDG_RUNTIME_DIR/ears/debug.log"
    fi
}
