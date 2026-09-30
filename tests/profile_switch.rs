//! A subprocess keeps configuration environment changes away from other tests.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::{tui::App, Config};

#[test]
fn profile_switch_preserves_settings_on_failure() {
    if std::env::var_os("EARS_PROFILE_SWITCH_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "profile_switch_preserves_settings_on_failure",
                "--nocapture",
            ])
            .env("EARS_PROFILE_SWITCH_TEST", "1")
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_RUNTIME_DIR", root.path().join("run"));
        for (key, _) in std::env::vars().filter(|(key, _)| key.starts_with("EARS_")) {
            command.env_remove(key);
        }
        command.env("EARS_PROFILE_SWITCH_TEST", "1");
        assert!(command.status().unwrap().success());
        return;
    }
    let config = Config::new().unwrap();
    std::fs::create_dir_all(&config.config_dir).unwrap();
    let named = config.config_dir.join("config.broken.toml");
    let selection = config.config_dir.join("profile");
    std::fs::write(
        config.config_dir.join("config.toml"),
        "device = 'original'\n",
    )
    .unwrap();
    std::fs::write(&named, "[broken").unwrap();
    let mut app = App::with_profile(Some("")).unwrap();
    app.available_profiles = vec!["broken".into()];
    let cycle = |app: &mut App| {
        app.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT))
            .unwrap();
    };
    cycle(&mut app);
    assert_eq!(app.device, "original");
    assert_eq!(app.profile, None);
    assert!(!selection.exists());
    assert!(app.logs.iter().any(|s| s.contains("Profile switch failed")));

    std::fs::write(&named, "device = 'replacement'\n").unwrap();
    // A directory in place of the selection file forces a persistence failure.
    std::fs::create_dir(&selection).unwrap();
    cycle(&mut app);
    assert_eq!(app.device, "original");
    assert_eq!(app.profile, None);
    assert!(app
        .logs
        .iter()
        .any(|s| s.contains("Cannot persist profile switch")));
    std::fs::remove_dir(&selection).unwrap();

    cycle(&mut app);
    assert_eq!(app.device, "replacement");
    assert_eq!(app.profile.as_deref(), Some("broken"));
    assert_eq!(
        Config::get_default_profile().unwrap().as_deref(),
        Some("broken")
    );
    cycle(&mut app);
    assert_eq!(app.device, "original");
    assert_eq!(app.profile, None);
    assert_eq!(Config::get_default_profile().unwrap(), None);
}
