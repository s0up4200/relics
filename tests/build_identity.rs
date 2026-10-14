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

#[test]
fn a_cached_build_script_reads_the_archive_commit_each_time_it_runs() {
    let dir = std::env::temp_dir().join(format!("relics-build-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let script = dir.join("build-script");
    let compiled = Command::new("rustc")
        .args(["--edition=2024", concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"), "-o"])
        .arg(&script)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    for (metadata, expected) in [
        ("0123456789abcdef0123456789abcdef01234567", "0123456789ab"),
        ("abcdef0123456789abcdef0123456789abcdef01", "abcdef012345"),
        ("$Format:%H$", "unknown"),
    ] {
        std::fs::write(dir.join(".git-revision"), metadata).unwrap();
        let output = Command::new(&script).current_dir(&dir).output().unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains(&format!("cargo::rustc-env=RELICS_REVISION={expected}\n")), "{output}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
