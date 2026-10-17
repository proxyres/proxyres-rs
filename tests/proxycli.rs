use std::process::{Command, Output};

fn proxycli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proxycli"))
        .args(args)
        .output()
        .expect("unable to run proxycli")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

#[test]
fn help() {
    let output = proxycli(&[]);
    assert!(!output.status.success());
    assert!(stdout(&output).starts_with("proxycli"));
}

#[test]
fn config() {
    let output = proxycli(&["config"]);
    assert!(output.status.success());
    let out = stdout(&output);
    assert!(out.starts_with("Proxy configuration"));
    assert!(out.contains("Auto discover: "));
    assert!(out.contains("Proxy bypass: "));
}

#[test]
fn config_auto_discover() {
    let output = proxycli(&["config", "auto_discover"]);
    assert!(output.status.success());
    assert!(["enabled", "disabled"].contains(&stdout(&output).as_str()));
}

#[cfg(feature = "quickjs")]
#[test]
fn execute() {
    let pac = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pac.js");
    let output = proxycli(&[
        "execute",
        pac,
        "http://simple.com/",
        "https://multi.com/",
        "http://plain/",
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "PROXY no-such-proxy:80\nHTTPS some-such-proxy:443;HTTPS any-such-proxy:41\nDIRECT"
    );
}

#[test]
fn execute_missing_script() {
    let output = proxycli(&["execute", "does-not-exist.js", "http://simple.com/"]);
    assert!(!output.status.success());
}
