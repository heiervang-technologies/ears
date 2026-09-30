//! Asynchronous hooks own their audio copy even after the CLI exits.
use anyhow::{Context, Result};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn run(audio: &Path, text: &str) {
    let Some(dirs) = directories::ProjectDirs::from("com", "heiervang", "ears") else {
        return;
    };
    let hook = dirs.config_dir().join("hooks/post-transcribe");
    if !std::fs::metadata(&hook).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
        return;
    }
    // Spawn before returning: a short-lived CLI must not exit before a detached
    // thread gets scheduled. The child owns cleanup, not the parent's thread.
    match spawn_hook(&hook, audio, text) {
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(status) if status.success() => tracing::debug!("Post-transcribe hook completed"),
                Ok(status) => tracing::warn!("Post-transcribe hook failed: {}", status),
                Err(error) => tracing::warn!("Failed to wait for post-transcribe hook: {}", error),
            });
        }
        Err(error) => tracing::warn!("Failed to start post-transcribe hook: {:#}", error),
    }
}

fn spawn_hook(hook: &Path, audio: &Path, text: &str) -> Result<Child> {
    let parent = audio
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut copy = tempfile::Builder::new()
        .prefix("hook-")
        .suffix(".wav")
        .tempfile_in(parent)
        .context("Failed to create hook audio copy")?;
    let mut source = std::fs::File::open(audio).context("Failed to open hook source audio")?;
    std::io::copy(&mut source, &mut copy).context("Failed to copy hook audio")?;
    let (file, path) = copy.keep().context("Failed to retain hook audio")?;
    drop(file);
    // Paths and transcript are positional arguments, never interpolated shell
    // code. The supervisor survives the CLI and removes the copy on normal or
    // failed hook exit (including a missing/non-executable hook).
    let result = Command::new("/bin/sh")
        .args([
            "-c",
            "trap '/usr/bin/rm -f -- \"$1\"' 0; \"$2\" \"$1\" \"$3\"",
            "ears-post-transcribe",
        ])
        .arg(&path)
        .arg(hook)
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match result {
        Ok(child) => Ok(child),
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_file(&path) {
                tracing::warn!(
                    "Failed to remove unused hook audio {}: {}",
                    path.display(),
                    cleanup
                );
            }
            Err(error).context("Failed to spawn hook supervisor")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fixture(root: &Path, exit: i32, delay: bool) -> (std::path::PathBuf, std::path::PathBuf) {
        let hook = root.join("hook with ' quotes");
        let audio = root.join("original.wav");
        std::fs::write(&audio, b"original audio bytes").unwrap();
        let delay = if delay { "/usr/bin/sleep 0.2\n" } else { "" };
        std::fs::write(&hook, format!("#!/bin/sh\n{delay}printf '%s' \"$1\" > \"$0.path\"\n/usr/bin/cat -- \"$1\" > \"$0.audio\"\nprintf '%s' \"$2\" > \"$0.text\"\nexit {exit}\n")).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        (hook, audio)
    }

    #[test]
    fn cleanup_on_success_failure_and_missing_hook() {
        let root = tempfile::tempdir().unwrap();
        for exit in [0, 7] {
            let (hook, audio) = fixture(root.path(), exit, false);
            let text = "quotes ' \" $(touch injected) `id` 日本語\nline two";
            let status = spawn_hook(&hook, &audio, text).unwrap().wait().unwrap();
            assert_eq!(status.code(), Some(exit));
            assert_eq!(
                std::fs::read(hook.with_extension("audio")).unwrap(),
                b"original audio bytes"
            );
            assert_eq!(
                std::fs::read_to_string(hook.with_extension("text")).unwrap(),
                text
            );
            let copy = std::fs::read_to_string(hook.with_extension("path")).unwrap();
            assert!(!Path::new(&copy).exists());
            assert!(audio.exists());
        }
        let audio = root.path().join("original.wav");
        assert!(!spawn_hook(&root.path().join("missing"), &audio, "")
            .unwrap()
            .wait()
            .unwrap()
            .success());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("hook-")));
    }

    #[test]
    fn hook_finishes_and_cleans_after_launcher_exits() {
        const MARKER: &str = "EARS_HOOK_TEST_ROOT";
        if let Some(root) = std::env::var_os(MARKER) {
            let root = Path::new(&root);
            // Intentionally let the launcher exit immediately after spawning.
            drop(
                spawn_hook(
                    &root.join("hook with ' quotes"),
                    &root.join("original.wav"),
                    "after exit",
                )
                .unwrap(),
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let (hook, audio) = fixture(root.path(), 0, true);
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "post_transcribe::tests::hook_finishes_and_cleans_after_launcher_exits",
            ])
            .env(MARKER, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(copy) = std::fs::read_to_string(hook.with_extension("path")) {
                if !copy.is_empty() && !Path::new(&copy).exists() {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "hook failed to finish/clean up after parent exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(hook.with_extension("audio")).unwrap(),
            std::fs::read(audio).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(hook.with_extension("text")).unwrap(),
            "after exit"
        );
    }
}
