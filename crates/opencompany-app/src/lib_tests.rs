use super::*;

#[test]
fn desktop_defaults_to_dot_opencompany_under_home() {
    const CHILD: &str = "OPENCOMPANY_DESKTOP_DATA_DIR_TEST_CHILD";
    let home = PathBuf::from("/opencompany-test-home");

    if std::env::var_os(CHILD).is_some() {
        assert_eq!(default_data_dir(), home.join(".opencompany"));
        return;
    }

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("test::desktop_defaults_to_dot_opencompany_under_home")
        .env(CHILD, "1")
        .env("HOME", &home)
        .env_remove("USERPROFILE")
        .env_remove("OPENCOMPANY_DATA_DIR")
        .status()
        .unwrap();

    assert!(status.success());
}

/// **B1: the desktop build compiles the analytics transport.** Without it every
/// desktop would resolve to "reporting" and then install a no-op, which is the
/// quietest possible way to ship no analytics at all.
#[test]
fn the_desktop_build_has_the_analytics_transport() {
    assert!(opencompany::analytics::BuildFlags::of_this_build().analytics);
}
