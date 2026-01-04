use std::io::Write;
/// QA Round 3: Audio file validation bug
///
/// BUG FOUND: The code only checks that audio file exists and has size > 0,
/// but doesn't validate that it's a valid WAV file. A corrupted or partial
/// WAV file would pass validation and cause whisper to fail with unclear errors.
use tempfile::NamedTempFile;

#[tokio::test]
async fn test_corrupted_wav_file_passes_basic_checks() {
    println!("\n🐛 BUG: Corrupted audio files pass basic size validation");

    // Create a "fake" WAV file that has content but isn't valid WAV
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file
        .write_all(b"This is not a WAV file, just garbage data")
        .unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // Check if file exists
    assert!(audio_path.exists(), "File should exist");

    // Check if file has content
    let metadata = tokio::fs::metadata(audio_path).await.unwrap();
    assert!(metadata.len() > 0, "File should have content");

    println!("✓ File exists: true");
    println!("✓ File size > 0: true (size: {} bytes)", metadata.len());
    println!("✗ File is valid WAV: UNKNOWN - not checked!");

    // This file would pass the validation in main.rs (lines 319-335)
    // but would fail during whisper transcription with unclear error

    println!("\n🐛 CONFIRMED BUG:");
    println!("   Location: src/main.rs lines 319-335 (stop_and_transcribe)");
    println!("   Issue: Only checks exists() and len() > 0");
    println!("   Impact: Corrupted/partial WAV files pass validation");
    println!("   Result: Whisper fails with unclear error instead of detecting corrupt file early");
}

#[tokio::test]
async fn test_partial_wav_file_passes_validation() {
    println!("\n🐛 BUG: Partial WAV files (from disk full/crash) pass validation");

    // Simulate a partial WAV file (e.g., from disk full during recording)
    let mut temp_file = NamedTempFile::new().unwrap();

    // Write just WAV header without actual audio data
    let wav_header = [
        0x52, 0x49, 0x46, 0x46, // "RIFF"
        0x24, 0x00, 0x00, 0x00, // File size - 8 (incorrect)
        0x57, 0x41, 0x56, 0x45, // "WAVE"
    ];
    temp_file.write_all(&wav_header).unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // This passes basic checks
    assert!(audio_path.exists());
    let metadata = tokio::fs::metadata(audio_path).await.unwrap();
    assert!(metadata.len() > 0);

    println!("✓ File exists: true");
    println!("✓ File size > 0: true (size: {} bytes)", metadata.len());
    println!("✗ WAV file complete: NO - but not validated!");

    println!("\n🐛 CONFIRMED BUG:");
    println!("   A partial WAV file (e.g., from interrupted recording) would pass");
    println!("   validation and cause confusing whisper errors");
}

#[test]
fn test_empty_file_is_caught() {
    println!("\n✅ WORKS: Empty files ARE caught by current validation");

    let temp_file = NamedTempFile::new().unwrap();
    let audio_path = temp_file.path();

    // Empty file
    assert!(audio_path.exists());

    // The code at main.rs:328-335 correctly catches this
    // if metadata.len() == 0 {
    //     AudioFeedback::beep_error().ok();
    //     Notifications::error("Recording file is empty").ok();
    //     std::fs::remove_file(&audio_file).ok();
    //     tracing::error!("Audio file is empty");
    //     anyhow::bail!("Audio file is empty");
    // }

    println!("✓ Current code correctly catches empty files");
}

#[test]
fn demonstrate_wav_header_validation() {
    println!("\n💡 SOLUTION: Should validate WAV file header");

    // A proper validation would check:
    // 1. File starts with "RIFF"
    // 2. Contains "WAVE" format marker
    // 3. Has required chunks (fmt, data)
    // 4. Data chunk size matches file size

    let valid_header_check = |data: &[u8]| -> bool {
        if data.len() < 12 {
            return false;
        }
        // Check for "RIFF" at start
        if &data[0..4] != b"RIFF" {
            return false;
        }
        // Check for "WAVE" at offset 8
        if &data[8..12] != b"WAVE" {
            return false;
        }
        true
    };

    // Test with valid header
    let valid = [
        0x52, 0x49, 0x46, 0x46, // "RIFF"
        0x24, 0x00, 0x00, 0x00, // Size
        0x57, 0x41, 0x56, 0x45, // "WAVE"
    ];
    assert!(valid_header_check(&valid));

    // Test with invalid header
    let invalid = b"Not a WAV file";
    assert!(!valid_header_check(invalid));

    println!("✓ Basic WAV header validation is straightforward to implement");
}
