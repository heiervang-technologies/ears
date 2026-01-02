#!/usr/bin/env bats
#
# Unit tests for device listing and selection
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

@test "list devices shows audio sources only" {
    run "$EARS_BIN" --list
    assert_success
    assert_output_contains "Built-in Audio Analog Stereo"
    assert_output_contains "HyperX Cloud II Wireless"
}

@test "list devices excludes video sources" {
    run "$EARS_BIN" --list
    assert_success
    ! echo "$output" | grep -q "HD Webcam"
}

@test "list devices excludes audio sinks" {
    run "$EARS_BIN" --list
    assert_success
    ! echo "$output" | grep -q "Audio/Sink"
}

@test "list devices shows device IDs and descriptions" {
    run "$EARS_BIN" --list
    assert_success
    assert_output_contains "alsa_input.pci-0000_00_1f.3.analog-stereo"
    assert_output_contains "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
}

@test "short flag works for list" {
    run "$EARS_BIN" -l
    assert_success
    assert_output_contains "Built-in Audio"
}

@test "select device with fzf saves to config" {
    export MOCK_FZF_SELECTION="alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback	HyperX Cloud II Wireless"
    run "$EARS_BIN" --select
    assert_success
    assert_file_exists "$HOME/.config/ears/device"
    content=$(cat "$HOME/.config/ears/device")
    [[ "$content" == "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback" ]]
}

@test "select device shows confirmation" {
    export MOCK_FZF_SELECTION="alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback	HyperX Cloud II Wireless"
    run "$EARS_BIN" --select
    assert_success
    assert_output_contains "Selected: HyperX Cloud II Wireless"
    assert_output_contains "Device ID: alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
}

@test "select device handles cancellation" {
    export MOCK_FZF_CANCEL=1
    run "$EARS_BIN" --select
    assert_failure
}

@test "current device shows config file path" {
    echo "test-device" > "$HOME/.config/ears/device"
    run "$EARS_BIN" --current
    assert_success
    assert_output_contains "Config file:"
}
