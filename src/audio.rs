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

/// List audio sources from PipeWire's structured snapshot.
pub fn list_devices() -> Result<Vec<AudioDevice>> {
    let mut command = Command::new("pw-dump");
    command.stdin(Stdio::null()).stderr(Stdio::null());
    let output = crate::desktop::output_bounded(command, std::time::Duration::from_secs(2))
        .context("Failed to query PipeWire devices")?;
    if !output.status.success() {
        anyhow::bail!("pw-dump failed with status: {}", output.status);
    }
    parse_pw_dump_output(&output.stdout)
}

fn parse_pw_dump_output(output: &[u8]) -> Result<Vec<AudioDevice>> {
    let nodes: Vec<serde_json::Value> =
        serde_json::from_slice(output).context("Invalid PipeWire JSON snapshot")?;
    Ok(nodes
        .iter()
        .filter_map(|node| {
            if node.get("type")?.as_str()? != "PipeWire:Interface:Node" {
                return None;
            }
            let props = node.get("info")?.get("props")?;
            if props.get("media.class")?.as_str()? != "Audio/Source" {
                return None;
            }
            let name = props.get("node.name")?.as_str()?;
            if name.is_empty() {
                return None;
            }
            Some(AudioDevice {
                name: name.to_string(),
                description: props
                    .get("node.description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect())
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

    let output = child.wait_with_output().context("Failed to wait for fzf")?;

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

    #[test]
    fn parses_sources_and_escaped_names_from_json() {
        let snapshot = serde_json::json!([
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"usb\\mic","node.description":"Mic \"studio\" – 北"}}},
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Sink","node.name":"output"}}},
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"second"}}},
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":""}}},
            {"type":"PipeWire:Interface:Client","info":{"props":{"media.class":"Audio/Source","node.name":"not-a-node"}}},
            {"type":"PipeWire:Interface:Node","info":null}
        ]);
        let devices = parse_pw_dump_output(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(
            devices,
            vec![
                AudioDevice {
                    name: "usb\\mic".into(),
                    description: "Mic \"studio\" – 北".into()
                },
                AudioDevice {
                    name: "second".into(),
                    description: String::new()
                }
            ]
        );
    }

    #[test]
    fn empty_snapshot_is_distinct_from_invalid_output() {
        assert!(parse_pw_dump_output(b"[]").unwrap().is_empty());
        for invalid in [b"".as_slice(), b"{}", b"not json", b"[{"] {
            assert!(parse_pw_dump_output(invalid).is_err());
        }
    }
}
