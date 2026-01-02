use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// Represents an audio input device
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    /// Device node name (e.g., "alsa_input.usb-...")
    pub name: String,
    /// Human-readable description
    pub description: String,
}

/// List all available audio input sources via PipeWire
///
/// Parses `pw-cli ls Node` output to find devices with media.class = "Audio/Source"
pub fn list_devices() -> Result<Vec<AudioDevice>> {
    let output = Command::new("pw-cli")
        .arg("ls")
        .arg("Node")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("Failed to execute pw-cli")?;

    if !output.status.success() {
        anyhow::bail!("pw-cli failed with status: {}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_pw_cli_output(&stdout)
}

/// Parse pw-cli output to extract audio source devices
fn parse_pw_cli_output(output: &str) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();
    let mut current_device: Option<AudioDevice> = None;
    let mut is_source = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // New node detected
        if trimmed.starts_with("id ") && trimmed.contains(',') {
            // Save previous device if it was a source
            if is_source {
                if let Some(device) = current_device.take() {
                    if !device.name.is_empty() {
                        devices.push(device);
                    }
                }
            }
            // Reset state
            is_source = false;
            current_device = Some(AudioDevice {
                name: String::new(),
                description: String::new(),
            });
        }

        // Check if this node is an audio source
        if trimmed.contains(r#"media.class = "Audio/Source""#) {
            is_source = true;
        }

        // Extract node name
        if let Some(name_start) = trimmed.find(r#"node.name = ""#) {
            if let Some(ref mut device) = current_device {
                let name_str = &trimmed[name_start + 13..]; // Skip 'node.name = "'
                if let Some(end_quote) = name_str.find('"') {
                    device.name = name_str[..end_quote].to_string();
                }
            }
        }

        // Extract node description
        if let Some(desc_start) = trimmed.find(r#"node.description = ""#) {
            if let Some(ref mut device) = current_device {
                let desc_str = &trimmed[desc_start + 20..]; // Skip 'node.description = "'
                if let Some(end_quote) = desc_str.find('"') {
                    device.description = desc_str[..end_quote].to_string();
                }
            }
        }
    }

    // Don't forget the last device
    if is_source {
        if let Some(device) = current_device {
            if !device.name.is_empty() {
                devices.push(device);
            }
        }
    }

    Ok(devices)
}

/// Display devices in a formatted list
pub fn format_device_list(devices: &[AudioDevice]) -> String {
    if devices.is_empty() {
        return String::from("No audio input devices found");
    }

    devices
        .iter()
        .map(|d| format!("{}\t{}", d.name, d.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Select a device interactively using fzf
///
/// Returns the selected device name, or None if selection was cancelled
pub fn select_device_interactive(devices: &[AudioDevice]) -> Result<Option<String>> {
    if devices.is_empty() {
        anyhow::bail!("No audio input devices found");
    }

    // Format devices for fzf (description shown, name hidden in first column)
    let fzf_input = devices
        .iter()
        .map(|d| format!("{}\t{}", d.name, d.description))
        .collect::<Vec<_>>()
        .join("\n");

    let mut child = Command::new("fzf")
        .arg("--prompt=Select audio device: ")
        .arg("--with-nth=2")
        .arg("--delimiter=\t")
        .arg("--height=~50%")
        .arg("--border")
        .arg("--header=Audio Input Devices")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn fzf (is it installed?)")?;

    // Write input to fzf
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(fzf_input.as_bytes())
            .context("Failed to write to fzf stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for fzf")?;

    if !output.status.success() {
        // User cancelled selection
        return Ok(None);
    }

    let selection = String::from_utf8_lossy(&output.stdout);
    let selection = selection.trim();

    if selection.is_empty() {
        return Ok(None);
    }

    // Extract device name (first column before tab)
    let device_name = selection
        .split('\t')
        .next()
        .context("Invalid fzf output format")?
        .to_string();

    Ok(Some(device_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pw_cli_output() {
        let sample_output = r#"
        id 42, type PipeWire:Interface:Node/3
                object.serial = "42"
                object.path = "alsa:pcm:0:front:0:capture"
                node.name = "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
                node.description = "HyperX Cloud II Wireless Mono"
                media.class = "Audio/Source"
                node.nick = "HyperX Cloud II Wireless"

        id 43, type PipeWire:Interface:Node/3
                object.serial = "43"
                object.path = "alsa:pcm:1:front:1:capture"
                node.name = "alsa_input.pci-0000_00_1f.3.analog-stereo"
                node.description = "Built-in Audio Analog Stereo"
                media.class = "Audio/Source"
                node.nick = "Built-in Audio"

        id 44, type PipeWire:Interface:Node/3
                object.serial = "44"
                node.name = "some_output_device"
                node.description = "Some Output Device"
                media.class = "Audio/Sink"
        "#;

        let devices = parse_pw_cli_output(sample_output).unwrap();
        assert_eq!(devices.len(), 2);

        assert_eq!(
            devices[0].name,
            "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
        );
        assert_eq!(devices[0].description, "HyperX Cloud II Wireless Mono");

        assert_eq!(devices[1].name, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(devices[1].description, "Built-in Audio Analog Stereo");
    }

    #[test]
    fn test_parse_pw_cli_output_empty() {
        let empty_output = "";
        let devices = parse_pw_cli_output(empty_output).unwrap();
        assert_eq!(devices.len(), 0);
    }

    #[test]
    fn test_format_device_list() {
        let devices = vec![
            AudioDevice {
                name: "device1".to_string(),
                description: "Device One".to_string(),
            },
            AudioDevice {
                name: "device2".to_string(),
                description: "Device Two".to_string(),
            },
        ];

        let formatted = format_device_list(&devices);
        assert!(formatted.contains("device1\tDevice One"));
        assert!(formatted.contains("device2\tDevice Two"));
    }

    #[test]
    fn test_format_device_list_empty() {
        let devices = vec![];
        let formatted = format_device_list(&devices);
        assert_eq!(formatted, "No audio input devices found");
    }
}
