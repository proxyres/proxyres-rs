fn main() {
    println!("cargo:rerun-if-changed=src/log.c");
    println!("cargo:rerun-if-changed=tests/log_callback.c");
    cc::Build::new().file("src/log.c").compile("proxyres_capi_log");
    // Only linked by the integration tests
    cc::Build::new()
        .file("tests/log_callback.c")
        .cargo_metadata(false)
        .compile("proxyres_capi_test_log");
}
