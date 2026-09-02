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
