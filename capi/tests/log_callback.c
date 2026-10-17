#include <stdarg.h>
#include <stdio.h>

static char last_message[256];

// Log callback used by the integration tests that formats messages like a C caller would
void proxyres_test_log_cb(const char *fmt, va_list args) {
    vsnprintf(last_message, sizeof(last_message), fmt, args);
}

const char *proxyres_test_last_message(void) {
    return last_message;
}
