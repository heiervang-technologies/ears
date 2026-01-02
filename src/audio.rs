//! Audio recording and device management
//!
//! This module will be fully implemented in Iteration 3.
//! For now, it contains placeholder types and stub implementations.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Audio device information
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device node name (e.g., "alsa_input.usb-...")
    pub name: String,
    /// Human-readable description
    pub description: String,
}

/// Audio device manager
pub struct DeviceManager;

impl DeviceManager {
    /// List available audio input devices
    pub fn list_devices() -> Result<Vec<AudioDevice>> {
        let output = Command::new("pw-cli")
            .arg("ls")
            .arg("Node")
            .output()
            .context("Failed to execute pw-cli")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let devices = Self::parse_pw_cli_output(&stdout)?;
        Ok(devices)
    }

    /// Parse pw-cli output to extract audio sources
    fn parse_pw_cli_output(output: &str) -> Result<Vec<AudioDevice>> {
        let mut devices = Vec::new();
        let mut current_node: Option<String> = None;
        let mut current_desc: Option<String> = None;
        let mut is_source = false;

        for line in output.lines() {
            let trimmed = line.trim();

            // Check if this is an audio source
            if trimmed.contains("media.class = \"Audio/Source\"") {
                is_source = true;
            }

            // Extract node name
            if let Some(name_start) = trimmed.find("node.name = \"") {
                let name = &trimmed[name_start + 13..];
                if let Some(end) = name.find('"') {
                    current_node = Some(name[..end].to_string());
                }
            }

            // Extract description
            if let Some(desc_start) = trimmed.find("node.description = \"") {
                let desc = &trimmed[desc_start + 20..];
                if let Some(end) = desc.find('"') {
                    current_desc = Some(desc[..end].to_string());
                }
            }

            // End of node entry - add if it's a source
            if trimmed.starts_with("id ") && is_source {
                if let (Some(name), Some(desc)) = (current_node.clone(), current_desc.clone()) {
                    devices.push(AudioDevice {
                        name,
                        description: desc,
                    });
                }
                // Reset for next node
                current_node = None;
                current_desc = None;
                is_source = false;
            }
        }

        // Handle last entry
        if is_source {
            if let (Some(name), Some(desc)) = (current_node, current_desc) {
                devices.push(AudioDevice {
                    name,
                    description: desc,
                });
            }
        }

        Ok(devices)
    }

    /// Select device interactively using fzf
    pub fn select_device_interactive(devices: &[AudioDevice]) -> Result<Option<String>> {
        if devices.is_empty() {
            return Ok(None);
        }

        // Format devices for fzf: "name<TAB>description"
        let input: String = devices
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
            .spawn()
            .context("Failed to spawn fzf")?;

        // Write input to fzf
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .context("Failed to write to fzf stdin")?;
        }

        let output = child.wait_with_output().context("Failed to wait for fzf")?;

        if !output.status.success() {
            return Ok(None);
        }

        let selection = String::from_utf8_lossy(&output.stdout);
        let device_name = selection.trim().split('\t').next().map(|s| s.to_string());

        Ok(device_name)
    }
}

/// Audio recorder
pub struct Recorder {
    /// The spawned pw-record process
    process: Child,
}

impl Recorder {
    /// Start recording to a file
    pub fn start(device: &str, output_path: &Path, timeout_secs: u64) -> Result<Self> {
        let process = Command::new("timeout")
            .arg(timeout_secs.to_string())
            .arg("pw-record")
            .arg("--target")
            .arg(device)
            .arg("--rate")
            .arg("16000")
            .arg("--channels")
            .arg("1")
            .arg("--format")
            .arg("s16")
            .arg(output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn pw-record")?;

        Ok(Self { process })
    }

    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.process.id()
    }

    /// Stop the recording
    pub fn stop(self) -> Result<()> {
        // Kill the process
        let pid = self.process.id() as i32;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_output() {
        let devices = DeviceManager::parse_pw_cli_output("").unwrap();
        assert_eq!(devices.len(), 0);
    }

    #[test]
    fn test_parse_pw_cli_sample() {
        let sample = r#"
        id 42, type PipeWire:Interface:Node/3
            node.name = "alsa_input.usb-test"
            node.description = "Test Microphone"
            media.class = "Audio/Source"
        "#;

        let devices = DeviceManager::parse_pw_cli_output(sample).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "alsa_input.usb-test");
        assert_eq!(devices[0].description, "Test Microphone");
    }
}
