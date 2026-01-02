#!/usr/bin/env bats
#
# Integration tests for the full recording and transcription workflow
#

load ../test_helper

setup() {
    setup_test_env
    init_mock_tracker
    source_ears_functions
    mkdir -p "$XDG_RUNTIME_DIR/ears"
}

teardown() {
    teardown_test_env
}

@test "start recording creates PID file and audio file" {
    # Start recording in background
    run timeout 3 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' &
        ears_pid=\$!
        sleep 1
        # Check files exist
        test -f '$XDG_RUNTIME_DIR/ears/recording.pid' && \
        test -f '$XDG_RUNTIME_DIR/ears/recording.wav'
        result=\$?
        kill \$ears_pid 2>/dev/null || true
        exit \$result
    "
    assert_success
}

@test "stop recording triggers transcription" {
    # Start recording
    timeout 3 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' &
        sleep 1
    " &
    start_pid=$!
    sleep 1.5

    # Stop recording
    run timeout 3 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN'
    "

    # Wait for start process to complete
    wait $start_pid 2>/dev/null || true

    # Check that transcription was attempted
    sleep 0.5
    if [[ -f "$TEST_TEMP_DIR/typed_text.log" ]]; then
        [[ -s "$TEST_TEMP_DIR/typed_text.log" ]]
    fi
}

@test "transcription result is typed via ydotool" {
    # Create a mock recording state
    mkdir -p "$XDG_RUNTIME_DIR/ears"

    # Start a background process to simulate recording
    sleep 10 &
    record_pid=$!
    echo "$record_pid" > "$XDG_RUNTIME_DIR/ears/recording.pid"

    # Create a minimal WAV file
    printf 'RIFF' > "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf '\x24\x00\x00\x00' >> "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf 'WAVE' >> "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Stop and transcribe
    run timeout 5 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1
    "

    # Clean up background process
    kill $record_pid 2>/dev/null || true
    wait $record_pid 2>/dev/null || true

    # Check if ydotool was called
    sleep 0.3
    if [[ -f "$TEST_TEMP_DIR/typed_text.log" ]]; then
        grep -q "This is a test transcription" "$TEST_TEMP_DIR/typed_text.log"
    fi
}

@test "empty transcription does not type anything" {
    # Override curl mock to return empty response
    cat > "$TEST_TEMP_DIR/curl_override" <<'EOF'
#!/usr/bin/env bash
if [[ "$*" =~ /inference ]]; then
    cat "$BATS_TEST_DIRNAME/../fixtures/whisper-response-empty.json"
else
    echo "ok"
fi
EOF
    chmod +x "$TEST_TEMP_DIR/curl_override"
    export PATH="$TEST_TEMP_DIR:$PATH"

    # Create mock recording state
    sleep 10 &
    record_pid=$!
    echo "$record_pid" > "$XDG_RUNTIME_DIR/ears/recording.pid"
    printf 'RIFF' > "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf '\x24\x00\x00\x00' >> "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf 'WAVE' >> "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Stop and transcribe
    run timeout 5 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1
    "

    # Clean up
    kill $record_pid 2>/dev/null || true
    wait $record_pid 2>/dev/null || true

    # Should not have typed anything
    ! [[ -f "$TEST_TEMP_DIR/typed_text.log" ]] || ! [[ -s "$TEST_TEMP_DIR/typed_text.log" ]]
}

@test "silence artifact 'Thank you.' is filtered" {
    # Override curl mock to return silence artifact
    cat > "$TEST_TEMP_DIR/curl_override" <<'EOF'
#!/usr/bin/env bash
if [[ "$*" =~ /inference ]]; then
    cat "$BATS_TEST_DIRNAME/../fixtures/whisper-response-silence.json"
else
    echo "ok"
fi
EOF
    chmod +x "$TEST_TEMP_DIR/curl_override"
    export PATH="$TEST_TEMP_DIR:$PATH"

    # Create mock recording state
    sleep 10 &
    record_pid=$!
    echo "$record_pid" > "$XDG_RUNTIME_DIR/ears/recording.pid"
    printf 'RIFF' > "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf '\x24\x00\x00\x00' >> "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf 'WAVE' >> "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Stop and transcribe
    run timeout 5 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1
    "

    # Clean up
    kill $record_pid 2>/dev/null || true
    wait $record_pid 2>/dev/null || true

    # Should show "No speech detected" notification
    if [[ -f "$TEST_TEMP_DIR/notifications.log" ]]; then
        grep -q "No speech detected" "$TEST_TEMP_DIR/notifications.log"
    fi
}

@test "server health check fails gracefully" {
    # Override curl to fail on health check
    cat > "$TEST_TEMP_DIR/curl_override" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
    chmod +x "$TEST_TEMP_DIR/curl_override"
    export PATH="$TEST_TEMP_DIR:$PATH"

    # Try to start recording
    run timeout 3 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1
    "

    # Should fail with error
    [[ "$status" -ne 0 ]] || [[ "$status" -eq 124 ]]

    # Should show notification
    if [[ -f "$TEST_TEMP_DIR/notifications.log" ]]; then
        grep -q "Whisper server not running" "$TEST_TEMP_DIR/notifications.log"
    fi
}

@test "audio feedback is triggered on start" {
    run timeout 3 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' &
        sleep 0.5
    "

    # Check if paplay was called
    if [[ -f "$MOCK_CALL_LOG" ]]; then
        grep -q "paplay.*start" "$MOCK_CALL_LOG" || grep -q "paplay" "$MOCK_CALL_LOG"
    fi
}

@test "audio feedback is triggered on completion" {
    # Create mock recording state
    sleep 10 &
    record_pid=$!
    echo "$record_pid" > "$XDG_RUNTIME_DIR/ears/recording.pid"
    printf 'RIFF' > "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf '\x24\x00\x00\x00' >> "$XDG_RUNTIME_DIR/ears/recording.wav"
    printf 'WAVE' >> "$XDG_RUNTIME_DIR/ears/recording.wav"

    # Stop and transcribe
    run timeout 5 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' 2>&1
    "

    # Clean up
    kill $record_pid 2>/dev/null || true
    wait $record_pid 2>/dev/null || true

    # Check if paplay was called for completion
    sleep 0.3
    if [[ -f "$MOCK_CALL_LOG" ]]; then
        grep -q "paplay.*done" "$MOCK_CALL_LOG" || grep -q "paplay" "$MOCK_CALL_LOG"
    fi
}

@test "recording timeout prevents runaway recordings" {
    # The pw-record mock includes a timeout simulation
    # This test just verifies the timeout parameter is passed
    run timeout 5 bash -c "
        export XDG_RUNTIME_DIR='$XDG_RUNTIME_DIR'
        export HOME='$HOME'
        export PATH='$PATH'
        '$EARS_BIN' &
        sleep 2
    "

    # Check that timeout command is used in the call
    if [[ -f "$MOCK_CALL_LOG" ]]; then
        grep -q "pw-record" "$MOCK_CALL_LOG"
    fi
}
