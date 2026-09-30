//! Exercise real probe entry points with private fake binaries, never the desktop.
use ears::{KeyboardLayout, Notifications, Urgency};
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

#[test]
fn desktop_probes_kill_and_reap_hung_helpers() {
    if std::env::var_os("EARS_HELPER_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        for name in ["hyprctl", "dconf", "notify-send", "pw-dump"] {
            let path = root.path().join(name);
            std::fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$HELPER_TEST_LOG\"\nexec /usr/bin/sleep 60\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "desktop_probes_kill_and_reap_hung_helpers",
                "--nocapture",
            ])
            .env("EARS_HELPER_TEST", "1")
            .env("PATH", root.path())
            .env("HELPER_TEST_LOG", root.path().join("pids"))
            .status()
            .unwrap();
        assert!(status.success());
        let pids = std::fs::read_to_string(root.path().join("pids")).unwrap();
        assert_eq!(pids.lines().count(), 4);
        for pid in pids.lines() {
            let pid: i32 = pid.parse().unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "helper {pid} survived");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        return;
    }
    let start = Instant::now();
    assert_eq!(KeyboardLayout::detect_language(), None);
    assert!(Notifications::send("test only", Urgency::Low).is_err());
    assert!(ears::audio::list_devices().is_err());
    assert!(start.elapsed() < Duration::from_secs(8));
}
