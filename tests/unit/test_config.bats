#!/usr/bin/env bats
#
# Unit tests for configuration management
#

load ../test_helper

setup() {
    setup_test_env
    init_mock_tracker
    source_ears_functions
}

teardown() {
    teardown_test_env
}

@test "default whisper server is localhost:8178" {
    run bash -c "source $EARS_BIN --server 2>&1 | grep -o 'http://127.0.0.1:8178'"
    assert_success
    [[ "$output" == "http://127.0.0.1:8178" ]]
}

@test "can set custom whisper server URL" {
    run "$EARS_BIN" --server "http://localhost:9000"
    assert_success
    assert_file_exists "$HOME/.config/ears/server"
    assert_file_contains "$HOME/.config/ears/server" "http://localhost:9000"
}

@test "can retrieve saved whisper server URL" {
    echo "http://custom-server:8080" > "$HOME/.config/ears/server"
    run "$EARS_BIN" --server
    assert_success
    assert_output_contains "http://custom-server:8080"
}

@test "server config is created in correct location" {
    run "$EARS_BIN" --server "http://example.com:8178"
    assert_success
    [[ -f "$HOME/.config/ears/server" ]]
}

@test "default device is used when no config exists" {
    run "$EARS_BIN" --current
    assert_success
    assert_output_contains "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
}

@test "can save device selection to config" {
    # Simulate device selection
    export MOCK_FZF_SELECTION="alsa_input.pci-0000_00_1f.3.analog-stereo	Built-in Audio Analog Stereo"
    run "$EARS_BIN" --select
    assert_success
    assert_file_exists "$HOME/.config/ears/device"
    assert_file_contains "$HOME/.config/ears/device" "alsa_input.pci-0000_00_1f.3.analog-stereo"
}

@test "device config shows saved device" {
    echo "alsa_input.test-device" > "$HOME/.config/ears/device"
    run "$EARS_BIN" --current
    assert_success
    assert_output_contains "alsa_input.test-device"
}

@test "help text is displayed correctly" {
    run "$EARS_BIN" --help
    assert_success
    assert_output_contains "Usage: ears"
    assert_output_contains "--select"
    assert_output_contains "--list"
    assert_output_contains "--server"
}

@test "short flags work for help" {
    run "$EARS_BIN" -h
    assert_success
    assert_output_contains "Usage: ears"
}
