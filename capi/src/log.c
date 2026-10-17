#include <stdarg.h>

typedef void (*proxyres_log_cb)(const char *fmt, va_list args);

static void proxyres_capi_log_va(proxyres_log_cb callback, const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    callback(fmt, args);
    va_end(args);
}

// Pass a formatted message to a callback that expects a format string and its arguments
void proxyres_capi_log(void *callback, const char *message) {
    proxyres_capi_log_va((proxyres_log_cb)callback, "%s", message);
}
