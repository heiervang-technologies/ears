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
    parse_pw_dump_output(&pw_dump()?)
}

fn pw_dump() -> Result<Vec<u8>> {
    let mut command = Command::new("pw-dump");
    command.stdin(Stdio::null()).stderr(Stdio::null());
    let output = crate::desktop::output_bounded(command, std::time::Duration::from_secs(2))
        .context("Failed to query PipeWire devices")?;
    if !output.status.success() {
        anyhow::bail!("pw-dump failed with status: {}", output.status);
    }
    Ok(output.stdout)
}

/// The audio sources PipeWire has right now, and which one is its default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    /// Node names of every `Audio/Source`.
    pub names: Vec<String>,
    /// Node name of the default source, when PipeWire reports one.
    pub default: Option<String>,
}

/// Query PipeWire for its current sources.
pub fn sources() -> Result<Sources> {
    parse_sources(&pw_dump()?)
}

fn parse_sources(output: &[u8]) -> Result<Sources> {
    let names = parse_pw_dump_output(output)?
        .into_iter()
        .map(|d| d.name)
        .collect();
    let nodes: Vec<serde_json::Value> =
        serde_json::from_slice(output).context("Invalid PipeWire JSON snapshot")?;
    let default = nodes.iter().find_map(|object| {
        if object.get("type")?.as_str()? != "PipeWire:Interface:Metadata" {
            return None;
        }
        object
            .get("metadata")?
            .as_array()?
            .iter()
            .find_map(|entry| {
                if entry.get("key")?.as_str()? != "default.audio.source" {
                    return None;
                }
                let name = entry.get("value")?.get("name")?.as_str()?;
                (!name.is_empty()).then(|| name.to_string())
            })
    });
    Ok(Sources { names, default })
}

/// Which source a capture will open, decided before `pw-record` starts.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureDevice {
    /// The device the configuration asked for.
    pub configured: String,
    /// What to pass to `pw-record --target`.
    pub target: String,
    /// The source expected to deliver the audio; `None` when unknown.
    pub opened: Option<String>,
    /// The configured device is not present in PipeWire.
    pub missing: bool,
}

impl CaptureDevice {
    /// The source for a log line or a report.
    pub fn opened_label(&self) -> String {
        match (&self.opened, self.missing && self.target == self.configured) {
            (Some(name), true) => format!("PipeWire default source ({name})"),
            (Some(name), false) => name.clone(),
            (None, true) => "PipeWire default source".to_string(),
            (None, false) => self.target.clone(),
        }
    }
}

/// `default` (or nothing) asks PipeWire for its default source.
fn is_default(device: &str) -> bool {
    device.is_empty() || device == "default"
}

/// Decide which source to capture from.
///
/// `sources` is `None` when PipeWire could not be asked; the configured device
/// is then used unverified. A configured device that is absent gives way to
/// the first of `fallbacks` that is present; with none present `pw-record`
/// still gets the configured name and PipeWire links its default source.
pub fn choose_capture_device(
    configured: &str,
    fallbacks: &[String],
    sources: Option<&Sources>,
) -> CaptureDevice {
    let choice = |target: &str, opened: Option<String>, missing| CaptureDevice {
        configured: configured.to_string(),
        target: target.to_string(),
        opened,
        missing,
    };
    let Some(sources) = sources else {
        return choice(configured, None, false);
    };
    if is_default(configured) {
        return choice(configured, sources.default.clone(), false);
    }
    // `--target` also takes an object serial, which names no node.
    let has = |name: &str| sources.names.iter().any(|n| n == name);
    if has(configured) || configured.parse::<u64>().is_ok() {
        return choice(configured, Some(configured.to_string()), false);
    }
    for fallback in fallbacks {
        if is_default(fallback) {
            break;
        }
        if has(fallback) {
            return choice(fallback, Some(fallback.clone()), true);
        }
    }
    choice(configured, sources.default.clone(), true)
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

    fn sources(names: &[&str], default: Option<&str>) -> Sources {
        Sources {
            names: names.iter().map(|n| n.to_string()).collect(),
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn reads_the_default_source_from_the_snapshot() {
        let snapshot = serde_json::json!([
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"rode"}}},
            {"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},"metadata":[
                {"subject":0,"key":"default.configured.audio.source","value":{"name":"jbl"}},
                {"subject":0,"key":"default.audio.source","value":{"name":"rode"}},
                {"subject":0,"key":"default.audio.sink","value":{"name":"speaker"}}
            ]}
        ]);
        assert_eq!(
            parse_sources(&serde_json::to_vec(&snapshot).unwrap()).unwrap(),
            sources(&["rode"], Some("rode"))
        );
        assert_eq!(parse_sources(b"[]").unwrap(), Sources::default());
        assert!(parse_sources(b"not json").is_err());
    }

    #[test]
    fn a_present_device_is_used_as_configured() {
        let pw = sources(&["ears_aec_mic", "rode"], Some("rode"));
        let choice = choose_capture_device("ears_aec_mic", &["rode".into()], Some(&pw));
        assert_eq!(choice.target, "ears_aec_mic");
        assert_eq!(choice.opened.as_deref(), Some("ears_aec_mic"));
        assert!(!choice.missing);
        assert_eq!(choice.opened_label(), "ears_aec_mic");
    }

    #[test]
    fn a_missing_device_gives_way_to_the_first_fallback_present() {
        let pw = sources(&["ears_aec_mic", "rode"], Some("rode"));
        let fallbacks = ["gone-too".to_string(), "ears_aec_mic".into(), "rode".into()];
        let choice = choose_capture_device("jbl", &fallbacks, Some(&pw));
        assert_eq!(choice.configured, "jbl");
        assert_eq!(choice.target, "ears_aec_mic");
        assert_eq!(choice.opened.as_deref(), Some("ears_aec_mic"));
        assert!(choice.missing);
        assert_eq!(choice.opened_label(), "ears_aec_mic");
    }

    #[test]
    fn with_nothing_present_the_default_source_is_named() {
        let pw = sources(&["rode"], Some("rode"));
        for fallbacks in [vec![], vec!["ears_aec_mic".to_string()]] {
            let choice = choose_capture_device("jbl", &fallbacks, Some(&pw));
            // As before the check existed: pw-record gets the configured name.
            assert_eq!(choice.target, "jbl");
            assert_eq!(choice.opened.as_deref(), Some("rode"));
            assert!(choice.missing);
            assert_eq!(choice.opened_label(), "PipeWire default source (rode)");
        }

        let choice = choose_capture_device("jbl", &[], Some(&sources(&[], None)));
        assert!(choice.missing);
        assert_eq!(choice.opened, None);
        assert_eq!(choice.opened_label(), "PipeWire default source");
    }

    #[test]
    fn a_default_in_the_fallbacks_ends_the_chain() {
        let pw = sources(&["rode", "later"], Some("rode"));
        let fallbacks = ["default".to_string(), "later".into()];
        let choice = choose_capture_device("jbl", &fallbacks, Some(&pw));
        assert_eq!(choice.target, "jbl");
        assert_eq!(choice.opened.as_deref(), Some("rode"));
        assert!(choice.missing);
    }

    #[test]
    fn the_default_device_and_an_unknown_pipewire_are_never_missing() {
        let pw = sources(&["rode"], Some("rode"));
        let choice = choose_capture_device("default", &["rode".into()], Some(&pw));
        assert_eq!(choice.target, "default");
        assert_eq!(choice.opened.as_deref(), Some("rode"));
        assert!(!choice.missing);

        // An object serial names no node; it cannot be checked by name.
        let choice = choose_capture_device("142", &[], Some(&pw));
        assert_eq!((choice.target.as_str(), choice.missing), ("142", false));

        // PipeWire could not be asked: nothing is claimed either way.
        let choice = choose_capture_device("jbl", &["rode".into()], None);
        assert_eq!(choice.target, "jbl");
        assert_eq!(choice.opened, None);
        assert!(!choice.missing);
        assert_eq!(choice.opened_label(), "jbl");
    }

    #[test]
    fn empty_snapshot_is_distinct_from_invalid_output() {
        assert!(parse_pw_dump_output(b"[]").unwrap().is_empty());
        for invalid in [b"".as_slice(), b"{}", b"not json", b"[{"] {
            assert!(parse_pw_dump_output(invalid).is_err());
        }
    }
}
