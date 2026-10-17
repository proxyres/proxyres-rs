#!/usr/bin/env bash
# Builds the C proxycli from the proxyres C library against the C API shared library.
set -eu
cd "$GITHUB_WORKSPACE"
cargo build -p proxyres-capi
out=target/debug
case "$RUNNER_OS" in
  Windows)
    # Share the C runtime heap with the library so strings it allocates can be freed
    clang -fms-runtime-lib=dll -D_CRT_SECURE_NO_WARNINGS -D_CRT_NONSTDC_NO_DEPRECATE -DPROXYRES_EXECUTE \
      -Icapi/include capi/tests/proxycli.c "$out/proxyres_capi.dll.lib" -o "$out/proxycli_c.exe"
    ;;
  *)
    cc -DPROXYRES_EXECUTE -Icapi/include capi/tests/proxycli.c -L"$out" -lproxyres_capi \
      -Wl,-rpath,"$GITHUB_WORKSPACE/$out" -o "$out/proxycli_c"
    ;;
esac
echo "PROXYCLI_C=$GITHUB_WORKSPACE/$out/proxycli_c" >> "$GITHUB_ENV"
