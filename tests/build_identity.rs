include!("../build.rs");

#[test]
fn version_exits_without_a_display_or_startup_log() {
    let output = Command::new(env!("CARGO_BIN_EXE_relics"))
        .arg("--version")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .expect("run the version command");
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("{}\n", relics::BUILD_IDENTITY));
    assert!(output.stderr.is_empty());
}
