use ears::Config;
use std::{fs, process::Command};

#[test]
fn profile_reads_report_errors_and_respect_precedence() {
    if std::env::var_os("EARS_CONFIG_IO_TEST").is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "profile_reads_report_errors_and_respect_precedence",
            ])
            .env("EARS_CONFIG_IO_TEST", "1")
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_RUNTIME_DIR", root.path().join("run"))
            .env_remove("EARS_PROFILE")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("ears");
    assert!(Config::get_default_profile().unwrap().is_none());
    assert!(Config::list_profiles().unwrap().is_empty());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), "device = 'default'\n").unwrap();
    fs::write(dir.join("config.named.toml"), "device = 'selected'\n").unwrap();
    fs::create_dir(dir.join("profile")).unwrap();
    assert!(Config::get_default_profile().is_err());
    assert!(Config::load_profile(None).is_err());
    assert_eq!(Config::load_profile(Some("")).unwrap().device, "default");
    std::env::set_var("EARS_PROFILE", "named");
    assert_eq!(Config::load_profile(None).unwrap().device, "selected");
    assert_eq!(Config::load_profile(Some("")).unwrap().device, "default");
    std::env::remove_var("EARS_PROFILE");
    fs::remove_dir(dir.join("profile")).unwrap();
    fs::write(dir.join("profile"), [0xff]).unwrap();
    assert!(Config::get_default_profile().is_err());
    assert!(Config::load_profile(None).is_err());
    fs::rename(&dir, dir.with_extension("saved")).unwrap();
    fs::write(&dir, "not a directory").unwrap();
    assert!(Config::list_profiles().is_err());
}
