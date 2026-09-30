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

#[test]
fn state_refresh_reaps_hung_helper_without_failing_persistence() {
    if std::env::var_os("EARS_STATE_HELPER_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        let helper = root.path().join("pkill");
        std::fs::write(
            &helper,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HELPER_TEST_LOG\"\nexec /usr/bin/sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "state_refresh_reaps_hung_helper_without_failing_persistence",
                "--nocapture",
            ])
            .env("EARS_STATE_HELPER_TEST", "1")
            .env("PATH", root.path())
            .env("HELPER_TEST_LOG", root.path().join("pid"))
            .status()
            .unwrap();
        assert!(status.success());
        let pid: i32 = std::fs::read_to_string(root.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "helper survived or was not reaped"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut manager = ears::state::StateManager::new(root.path()).unwrap();
    let start = Instant::now();
    manager.transition(ears::state::State::Recording).unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(
        std::fs::read_to_string(root.path().join("state")).unwrap(),
        "recording"
    );
}
