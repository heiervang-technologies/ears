//! Private fake helpers exercise paste without touching desktop input or clipboard.
use ears::{TextInput, TypingMode};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn paste_preserves_new_copies_and_aborts_on_copy_failure() {
    if std::env::var_os("EARS_CLIPBOARD_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        let scripts = [
            ("wl-copy", "printf 'copy\\n' >> \"$CLIP_LOG\"\n[ \"$COPY_FAIL\" = 1 ] && exit 1\nprintf '%s' \"$2\" > \"$CLIP_VALUE\"\n"),
            ("wl-paste", "printf 'read\\n' >> \"$CLIP_LOG\"\nprintf original\n"),
            ("ydotool", "printf 'paste\\n' >> \"$CLIP_LOG\"\nif [ \"$NEW_COPY\" = 1 ]; then printf new-user-copy > \"$CLIP_VALUE\"; fi\n"),
        ];
        for (name, body) in scripts {
            let path = root.path().join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "paste_preserves_new_copies_and_aborts_on_copy_failure",
            ])
            .env("EARS_CLIPBOARD_TEST", "1")
            .env("PATH", root.path())
            .env("CLIP_LOG", root.path().join("log"))
            .env("CLIP_VALUE", root.path().join("clipboard"))
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let log = std::env::var("CLIP_LOG").unwrap();
    let value = std::env::var("CLIP_VALUE").unwrap();
    for (new_copy, copy_fail, expected, calls) in [
        ("0", "0", "-- transcript é", "copy\npaste\n"),
        ("1", "0", "new-user-copy", "copy\npaste\n"),
        ("0", "1", "original", "copy\n"),
    ] {
        std::env::set_var("NEW_COPY", new_copy);
        std::env::set_var("COPY_FAIL", copy_fail);
        fs::write(&log, "").unwrap();
        fs::write(&value, "original").unwrap();
        let result = TextInput::type_text("-- transcript é", TypingMode::Paste);
        assert_eq!(result.is_err(), copy_fail == "1");
        assert_eq!(fs::read_to_string(&value).unwrap(), expected);
        assert_eq!(fs::read_to_string(&log).unwrap(), calls);
    }
}
