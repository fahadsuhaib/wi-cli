use std::process::{Command, Output};

fn run(config: &str, args: &[&str]) -> Output {
    let dir = std::env::temp_dir().join(format!("wicli-cli-test-{}-{}", std::process::id(), config.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wi"))
        .args(args)
        .env("WICLI_CONFIG", &path)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn check(out: &Output) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert!(err.contains("wi:"), "stderr: {err}");
    assert!(!err.contains("panicked"), "stderr: {err}");
}

#[test]
fn unknown_box_name_fails() {
    let cfg = "[[box]]\nname = \"a\"\nhost = \"127.0.0.1\"\nport = 1\nuser = \"u\"\nidentity = \"/nonexistent\"\n";
    check(&run(cfg, &["no-such-box"]));
}

#[test]
fn invalid_config_toml_fails() {
    check(&run("this is = [not valid toml", &[]));
    check(&run("this is = [not valid toml", &["somebox"]));
}

#[test]
fn version_flag_prints_version() {
    for flag in ["--version", "-V"] {
        let out = run("this is = [not valid toml", &[flag]); // runs before the config is read
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("wi {}", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn help_flag_prints_usage() {
    for flag in ["--help", "-h"] {
        let out = run("", &[flag]);
        assert_eq!(out.status.code(), Some(0));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("usage:") && text.contains("wi <folder>") && text.contains("wi fetch"), "{text}");
    }
}
