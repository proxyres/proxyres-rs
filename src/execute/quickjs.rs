//! PAC script execution using QuickJS.

use std::time::{Duration, Instant};

use rquickjs::convert::Coerced;
use rquickjs::function::Opt;
use rquickjs::{Context, Ctx, Function, IntoJs, Runtime, Value};

use super::{PAC_UTILS, natives};
use crate::Error;

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;

// Convert a QuickJS error into a script error, including the message of any thrown exception
fn script_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> Error {
    if !error.is_exception() {
        return Error::Script(error.to_string());
    }
    let exception: Value = ctx.catch();
    let message = if let Some(exception) = exception.as_exception() {
        exception.message().unwrap_or_else(|| "exception".to_string())
    } else if let Some(string) = exception.as_string() {
        string.to_string().unwrap_or_default()
    } else {
        format!("{exception:?}")
    };
    Error::Script(message)
}

// Resolve a host to its IPv4 address, or null if it cannot be resolved
fn dns_resolve<'js>(ctx: Ctx<'js>, host: Coerced<String>) -> rquickjs::Result<Value<'js>> {
    match natives::dns_resolve(&host) {
        Some(address) => address.into_js(&ctx),
        None => Ok(Value::new_null(ctx)),
    }
}

// Register native helper functions in the global object
fn register_natives(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    globals.set("dnsResolve", Function::new(ctx.clone(), dns_resolve)?)?;
    globals.set(
        "dnsResolveEx",
        Function::new(ctx.clone(), |host: Coerced<String>| natives::dns_resolve_ex(&host))?,
    )?;
    globals.set("myIpAddress", Function::new(ctx.clone(), natives::my_ip_address)?)?;
    globals.set("myIpAddressEx", Function::new(ctx.clone(), natives::my_ip_address_ex)?)?;
    globals.set(
        "alert",
        Function::new(ctx.clone(), |message: Opt<Coerced<String>>| {
            natives::alert(message.0.as_deref().map_or("", String::as_str))
        })?,
    )?;
    Ok(())
}

/// Evaluates the PAC script and calls `FindProxyForURL(url, host)`.
pub(super) fn find_proxy_for_url(script: &str, url: &str, host: &str, timeout: Duration) -> Result<String, Error> {
    let runtime = Runtime::new().map_err(|error| Error::Script(error.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);

    // Interrupt scripts that run too long
    let deadline = Instant::now() + timeout;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));

    let context = Context::full(&runtime).map_err(|error| Error::Script(error.to_string()))?;
    context.with(|ctx| {
        register_natives(&ctx).map_err(|error| script_error(&ctx, error))?;

        // Load Mozilla's PAC helper functions
        ctx.eval::<(), _>(PAC_UTILS)
            .map_err(|error| script_error(&ctx, error))?;

        // Evaluate the PAC script
        ctx.eval::<(), _>(script).map_err(|error| script_error(&ctx, error))?;

        let find_proxy_for_url: Value = ctx
            .globals()
            .get("FindProxyForURL")
            .map_err(|error| script_error(&ctx, error))?;
        let Some(find_proxy_for_url) = find_proxy_for_url.as_function() else {
            return Err(Error::Script("FindProxyForURL is not a function".to_string()));
        };

        let result: Value = find_proxy_for_url
            .call((url, host))
            .map_err(|error| script_error(&ctx, error))?;
        match result.as_string() {
            Some(list) => list.to_string().map_err(|error| script_error(&ctx, error)),
            None => Err(Error::Script(format!(
                "FindProxyForURL returned {}",
                result.type_name()
            ))),
        }
    })
}
