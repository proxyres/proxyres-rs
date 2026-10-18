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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn usage_without_command() {
    let output = proxycli(&[]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("Usage: proxycli"));
}

#[test]
fn help() {
    let output = proxycli(&["--help"]);
    assert!(output.status.success());
    let out = stdout(&output);
    for command in ["config", "execute", "resolve"] {
        assert!(out.contains(command), "missing {command} in {out}");
    }
}

#[test]
fn invalid_config_option() {
    let output = proxycli(&["config", "bogus"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("auto_discover"));
}

#[test]
fn resolve_requires_url() {
    assert!(!proxycli(&["resolve"]).status.success());
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

#[test]
fn config_verbose_after_command() {
    let output = proxycli(&["config", "auto_discover", "--verbose"]);
    assert!(output.status.success());
    assert!(["enabled", "disabled"].contains(&stdout(&output).as_str()));
}

#[test]
fn execute() {
    if !proxyres::execute::is_available() {
        return;
    }
    let pac = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/pac.js");
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

#[test]
fn resolve() {
    let output = proxycli(&["resolve", "http://localhost/", "http://127.0.0.1/"]);
    assert!(output.status.success(), "{output:?}");
    // Each url is printed before its proxies when there is more than one
    let out = stdout(&output);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("http://localhost/="), "{out}");
    assert!(lines[1].starts_with("http://127.0.0.1/="), "{out}");
}

#[test]
fn resolve_verbose() {
    let output = proxycli(&["--verbose", "resolve", "http://localhost/"]);
    assert!(output.status.success(), "{output:?}");
    let out = stdout(&output);
    assert!(
        out.starts_with("Resolving proxy for http://localhost/\n  Proxy: "),
        "{out}"
    );
}

#[test]
fn execute_verbose() {
    if !proxyres::execute::is_available() {
        return;
    }
    let script = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("proxycli_execute_verbose.js");
    std::fs::write(
        &script,
        r#"function FindProxyForURL(url, host) {
  alert("checking " + host);
  if (host == "fail") throw "failed";
  return "DIRECT";
}"#,
    )
    .unwrap();
    let script = script.to_str().unwrap();
    let output = proxycli(&["--verbose", "execute", script, "http://ok/", "http://fail/"]);
    // A url that fails makes the command fail after the other urls are printed
    assert!(!output.status.success());
    let out = stdout(&output);
    assert!(
        out.contains(&format!(
            "Executing PAC script {script} for http://ok/\n  Proxy: DIRECT"
        )),
        "{out}"
    );
    assert!(out.contains("Failed to execute PAC script for http://fail/"), "{out}");
    assert!(stderr(&output).contains("PAC alert: checking ok"), "{output:?}");
}
