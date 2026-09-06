use std::process::Command;

#[test]
fn prints_its_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_treff"))
        .arg("--version")
        .output()
        .expect("binary runs");
    assert!(out.status.success(), "exit status: {:?}", out.status);
    let text = String::from_utf8(out.stdout).expect("utf-8");
    assert!(
        text.trim().ends_with(env!("CARGO_PKG_VERSION")),
        "unexpected output: {text:?}"
    );
}

#[test]
fn refuses_to_start_without_configuration() {
    // Fail closed. A build that starts unconfigured would serve a forum with
    // no idea who anyone is; there is no sensible default for that.
    let out = Command::new(env!("CARGO_BIN_EXE_treff"))
        .env_clear()
        .output()
        .expect("binary runs");
    assert!(!out.status.success(), "must not start unconfigured");
}

#[test]
fn export_without_a_target_says_how_to_use_it() {
    // A backup command that does something surprising when called wrong is
    // worse than one that refuses.
    let out = Command::new(env!("CARGO_BIN_EXE_treff"))
        .arg("export")
        .env_clear()
        .output()
        .expect("binary runs");
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(text.contains("usage: treff export"), "got: {text}");
}

#[test]
fn an_unknown_command_is_refused() {
    let out = Command::new(env!("CARGO_BIN_EXE_treff"))
        .arg("frobnicate")
        .env_clear()
        .output()
        .expect("binary runs");
    assert!(!out.status.success());
}
