//! Validate correction helper arguments without sending desktop input.
use ears::progressive_typing::{ProgressiveTypingConfig, ProgressiveTypingEngine};
use ears::TypingMode;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn correction_backspaces_and_disabled_input() {
    if std::env::var_os("EARS_BACKSPACE_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        for name in ["wtype", "ydotool", "wl-copy", "wl-paste"] {
            let path = root.path().join(name);
            fs::write(&path, "#!/bin/sh\nprintf '%s' \"${0##*/}\" >> \"$KEY_LOG\"\nprintf ' <%s>' \"$@\" >> \"$KEY_LOG\"\nprintf '\\n' >> \"$KEY_LOG\"\n[ \"$KEY_FAIL\" != 1 ]\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "correction_backspaces_and_disabled_input"])
            .env("EARS_BACKSPACE_TEST", "1")
            .env("PATH", root.path())
            .env("KEY_LOG", root.path().join("keys"))
            .status()
            .unwrap()
            .success());
        return;
    }
    let log = std::env::var("KEY_LOG").unwrap();
    std::env::set_var("KEY_FAIL", "0");
    for mode in [TypingMode::Wtype, TypingMode::Paste, TypingMode::None] {
        let mut engine = ProgressiveTypingEngine::new(ProgressiveTypingConfig {
            enabled: true,
            auto_correction: true,
            typing_mode: mode,
        });
        engine.update("café👋").unwrap();
        fs::write(&log, "").unwrap();
        engine.update("caf").unwrap();
        let expected = match mode {
            TypingMode::Wtype => "wtype <-k> <BackSpace> <-k> <BackSpace>\n",
            TypingMode::Paste => "ydotool <key> <14:1> <14:0> <14:1> <14:0>\n",
            _ => "",
        };
        assert_eq!(fs::read_to_string(&log).unwrap(), expected);
        assert_eq!(engine.typed_text(), "caf");
        fs::write(&log, "").unwrap();
        engine.update("caf").unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "");
        engine.update("").unwrap();
        assert_eq!(engine.typed_text(), "");
    }
    let mut engine = ProgressiveTypingEngine::new(ProgressiveTypingConfig {
        enabled: true,
        auto_correction: true,
        typing_mode: TypingMode::Wtype,
    });
    engine.update("日本語").unwrap();
    std::env::set_var("KEY_FAIL", "1");
    assert!(engine.update("日本").is_err());
    assert_eq!(
        engine.typed_text(),
        "日本語",
        "failed correction must not advance state"
    );
}
