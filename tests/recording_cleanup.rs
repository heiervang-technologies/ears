//! Stop private fake recorders; never capture audio or drive desktop input.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn validation_errors_reset_state_and_clean_recordings() {
    for case in ["missing", "short", "invalid", "unreadable"] {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("run");
        let state = runtime.join("ears");
        let config = root.path().join("config/ears");
        let bin = root.path().join("bin");
        for dir in [&state, &config, &bin] {
            fs::create_dir_all(dir).unwrap();
        }
        for name in [
            "paplay",
            "notify-send",
            "pkill",
            "hyprctl",
            "dconf",
            "wtype",
            "ydotool",
            "wl-copy",
        ] {
            let path = bin.join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            config.join("config.toml"),
            "typing_mode = 'none'\nauto_enter = false\ncue_volume = 0\n",
        )
        .unwrap();
        let audio = state.join("recording.wav");
        match case {
            "short" => fs::write(&audio, b"short").unwrap(),
            "invalid" => fs::write(&audio, [0u8; 80]).unwrap(),
            "unreadable" => {
                fs::write(&audio, [0u8; 80]).unwrap();
                fs::set_permissions(&audio, fs::Permissions::from_mode(0o000)).unwrap();
            }
            _ => {}
        }
        let mut recorder = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        fs::write(state.join("recording.pid"), recorder.id().to_string()).unwrap();
        fs::write(state.join("state"), "recording").unwrap();
        let reaper = std::thread::spawn(move || recorder.wait().unwrap());
        let mut command = Command::new(env!("CARGO_BIN_EXE_ears"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("EARS_") {
                command.env_remove(key);
            }
        }
        let output = command
            .arg("toggle")
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("PATH", &bin)
            .output()
            .unwrap();
        reaper.join().unwrap();
        assert!(
            !output.status.success(),
            "{case}: expected validation failure"
        );
        assert_eq!(
            fs::read_to_string(state.join("state")).unwrap(),
            "idle",
            "{case}"
        );
        assert!(!audio.exists(), "{case}: recording survived failure");
        assert!(
            !state.join("recording.pid").exists(),
            "{case}: recorder PID survived"
        );
    }
}
