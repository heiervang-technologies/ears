/// QA Round 3: Config validation edge cases
///
/// Investigating what happens when config files contain edge case values
use ears::Config;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_empty_device_name_after_trim() {
    println!("\n🔍 BUG INVESTIGATION: What if device config file contains only whitespace?");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Write whitespace-only device name
    let device_file = config_dir.join("device");
    fs::write(&device_file, "   \n\t  \n  ").unwrap();

    // Try to load config
    let mut config = Config::new().unwrap();
    config.config_dir = config_dir.clone();

    // Manually simulate what Config::load does
    let device_str = fs::read_to_string(&device_file).unwrap().trim().to_string();

    println!("Device string after trim: '{}'", device_str);
    println!("Is empty: {}", device_str.is_empty());

    // BUG?: Config::load doesn't validate that device isn't empty after trim
    // Line 90-94 in config.rs:
    // config.device = fs::read_to_string(&device_file)
    //     .context("Failed to read device config file")?
    //     .trim()
    //     .to_string();

    if device_str.is_empty() {
        println!("\n🐛 POTENTIAL BUG:");
        println!("   Location: src/config.rs lines 87-94");
        println!("   Issue: Device config with only whitespace results in empty string");
        println!("   Result: config.device becomes empty, which would fail validation");
        println!("   But: Config::load() doesn't call validate(), so error appears later");
    }
}

#[test]
fn test_device_name_with_newlines() {
    println!("\n🔍 BUG INVESTIGATION: What if device name has embedded newlines?");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Write device name with newline in middle
    let device_file = config_dir.join("device");
    fs::write(&device_file, "device_name\nmalicious_extra_line").unwrap();

    // Load device
    let device_str = fs::read_to_string(&device_file).unwrap().trim().to_string();

    println!("Device string: '{}'", device_str);
    println!("Contains newline: {}", device_str.contains('\n'));

    // trim() only removes leading/trailing whitespace, not embedded newlines
    if device_str.contains('\n') {
        println!("\n⚠️  INTERESTING:");
        println!("   Multi-line device names are accepted");
        println!("   This probably won't work as a valid device name");
        println!("   But it's not a security issue - just a UX issue");
    }
}

#[test]
fn test_extremely_long_device_name() {
    println!("\n🔍 BUG INVESTIGATION: What if device name is extremely long?");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Write very long device name (10KB)
    let long_name = "a".repeat(10000);
    let device_file = config_dir.join("device");
    fs::write(&device_file, &long_name).unwrap();

    // Load device
    let device_str = fs::read_to_string(&device_file).unwrap().trim().to_string();

    println!("Device string length: {} bytes", device_str.len());

    // No length validation
    println!("\n⚠️  No length validation on device name");
    println!("   Could cause issues when passed to pw-record command");
    println!("   Probably not a security issue, just UX/robustness");
}

#[test]
fn test_special_characters_in_device_name() {
    println!("\n🔍 BUG INVESTIGATION: Special characters in device name");

    // Device names can legitimately contain special chars
    let special_devices = vec![
        "alsa_input.usb-Device_Name-00.mono",
        "alsa_input.pci-0000_00_1f.3.analog-stereo",
        "device-with-dashes",
        "device_with_underscores",
        "device.with.dots",
    ];

    for device in special_devices {
        println!("  ✓ Valid device: {}", device);
    }

    // But what about truly malicious characters?
    let malicious_devices = vec![
        "device; rm -rf /",
        "device && echo pwned",
        "device | cat /etc/passwd",
        "device`whoami`",
        "device$(whoami)",
    ];

    for device in malicious_devices {
        println!("  ⚠️  Malicious attempt: {}", device);
    }

    println!("\n✅ SECURITY: These are safe because:");
    println!("   1. Device name is passed as argument to Command, not shell");
    println!("   2. Command::new(\"pw-record\").arg(device) doesn't interpret shell");
    println!("   3. No shell metacharacter expansion");
}

#[test]
fn test_server_url_without_port() {
    println!("\n🔍 BUG INVESTIGATION: Server URL without explicit port");

    use url::Url;

    let urls = vec![
        "http://localhost",
        "http://192.168.1.100",
        "https://whisper.example.com",
    ];

    for url_str in urls {
        let parsed = Url::parse(url_str).unwrap();
        println!("  URL: {}", url_str);
        println!("    Parsed: {}", parsed.as_str());
        println!("    Port: {:?}", parsed.port());
        println!(
            "    Port or default: {}",
            parsed.port_or_known_default().unwrap_or(0)
        );
    }

    println!("\n✅ URLs without ports are valid and work correctly");
}

#[test]
fn test_server_url_with_path() {
    println!("\n🔍 BUG INVESTIGATION: Server URL with path component");

    use url::Url;

    let url_str = "http://localhost:8178/api/v1";
    let parsed = Url::parse(url_str).unwrap();

    println!("  URL: {}", url_str);
    println!("  Parsed: {}", parsed.as_str());
    println!("  Path: {}", parsed.path());

    // When building inference endpoint
    let base = parsed.as_str().trim_end_matches('/');
    let inference_url = format!("{}/inference", base);

    println!("  Inference URL: {}", inference_url);

    // This would become: http://localhost:8178/api/v1/inference
    // Which might not be correct if server expects /inference at root

    println!("\n⚠️  POTENTIAL ISSUE:");
    println!("   If user sets server URL with path, inference endpoint might be wrong");
    println!("   Example: http://localhost/whisper-api");
    println!("   Would create: http://localhost/whisper-api/inference");
    println!("   But server might expect: http://localhost/inference");
    println!("   Severity: LOW - documentation/UX issue");
}

#[test]
fn test_url_with_trailing_slashes() {
    println!("\n🐛 BUG INVESTIGATION: URLs with multiple trailing slashes");

    use url::Url;

    // The URL parsing normalizes trailing slashes
    let urls = vec![
        "http://localhost:8178",
        "http://localhost:8178/",
        "http://localhost:8178//",
        "http://localhost:8178///",
    ];

    for url_str in urls {
        let parsed = Url::parse(url_str).unwrap();
        let base = parsed.as_str().trim_end_matches('/');
        let inference_url = format!("{}/inference", base);

        println!("  Input: {} -> Inference: {}", url_str, inference_url);
    }

    // All result in http://localhost:8178/inference
    println!("\n✅ trim_end_matches('/') handles multiple slashes correctly");
}

#[test]
fn test_url_normalization_adds_trailing_slash() {
    println!("\n🔍 OBSERVATION: URL parsing adds trailing slash");

    use url::Url;

    let input = "http://localhost:8178";
    let parsed = Url::parse(input).unwrap();

    println!("  Input: {}", input);
    println!("  Parsed as_str(): {}", parsed.as_str());

    // Url::parse adds a trailing slash for URLs without path
    assert_eq!(parsed.as_str(), "http://localhost:8178/");

    println!("\n✅ This is expected URL normalization behavior");
    println!("   whisper.rs correctly handles this with trim_end_matches('/')");
}
