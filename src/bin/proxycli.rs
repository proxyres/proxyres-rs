//! Command line utility for reading proxy configuration and resolving proxies.

use std::process::ExitCode;

use proxyres::resolver::Resolver;
use proxyres::{config, execute};

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn or_not_set(value: Option<String>) -> String {
    value.unwrap_or_else(|| "not set".to_string())
}

fn print_proxy_config(options: &[String]) {
    let option = options.first().map(String::as_str);
    let all = option.is_none();
    if all {
        println!("Proxy configuration");
    }
    let label = |name: &str| if all { format!("  {name}: ") } else { String::new() };

    if all || option == Some("auto_discover") {
        let auto_discover = if config::auto_discover() { "enabled" } else { "disabled" };
        println!("{}{auto_discover}", label("Auto discover"));
    }
    if all || option == Some("auto_config_url") {
        println!("{}{}", label("Auto config url"), or_not_set(config::auto_config_url()));
    }
    if all || option == Some("proxy") {
        let scheme = options.get(1).map_or("http", String::as_str);
        println!("{}{}", label("Proxy"), or_not_set(config::proxy(scheme)));
    }
    if all || option == Some("bypass_list") {
        println!("{}{}", label("Proxy bypass"), or_not_set(config::bypass_list()));
    }
}

fn resolve_proxies_for_urls(urls: &[String], verbose: bool) -> bool {
    // Start asynchronous requests for all urls before waiting for any of them
    let mut resolvers = Vec::new();
    for url in urls {
        let mut resolver = Resolver::new();
        if let Err(error) = resolver.get_proxies_for_url(url) {
            println!("Unable to resolve proxy for {url} ({error})");
            return false;
        }
        resolvers.push(resolver);
    }

    let mut is_ok = true;
    for (url, resolver) in urls.iter().zip(resolvers.iter_mut()) {
        if verbose {
            println!("Resolving proxy for {url}");
        }
        resolver.wait(None);
        if verbose {
            print!("  Proxy: ");
        } else if urls.len() > 1 {
            print!("{url}=");
        }
        println!("{}", resolver.list().unwrap_or("direct://"));
        if let Some(error) = resolver.error() {
            println!("Unable to resolve proxy ({error})");
            is_ok = false;
        }
    }
    is_ok
}

fn execute_pac_script(script_path: &str, urls: &[String], verbose: bool) -> bool {
    let script = match std::fs::read_to_string(script_path) {
        Ok(script) => script,
        Err(error) => {
            println!("Failed to read PAC script {script_path} ({error})");
            return false;
        }
    };
    let mut is_ok = true;
    for url in urls {
        if verbose {
            println!("Executing PAC script {script_path} for {url}");
        }
        match execute::get_proxies_for_url(&script, url) {
            Ok(list) => {
                if verbose {
                    print!("  Proxy: ");
                }
                println!("{list}");
            }
            Err(error) => {
                println!("Failed to execute PAC script for {url} ({error})");
                is_ok = false;
            }
        }
    }
    is_ok
}

fn print_help() -> ExitCode {
    println!("proxycli [--help] [--verbose] cmd cmd_args");
    println!(" commands:");
    println!("  config [option]         - dumps proxy configuration values");
    println!("                            options: auto_discover, auto_config_url, proxy [scheme], bypass_list");
    println!("  execute [file] [urls..] - executes pac file with urls");
    println!("  resolve [url..]         - resolves proxy for urls");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let verbose = args.first().is_some_and(|arg| arg == "--verbose");
    if verbose {
        args.remove(0);
    }
    let _ = log::set_logger(&StderrLogger);
    log::set_max_level(if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Error
    });

    let Some(cmd) = args.first().cloned() else {
        return print_help();
    };
    let cmd_args = &args[1..];
    let is_ok = match cmd.as_str() {
        "config" => {
            print_proxy_config(cmd_args);
            true
        }
        "execute" if cmd_args.len() >= 2 => execute_pac_script(&cmd_args[0], &cmd_args[1..], verbose),
        "resolve" if !cmd_args.is_empty() => resolve_proxies_for_urls(cmd_args, verbose),
        "--help" => {
            print_help();
            true
        }
        _ => return print_help(),
    };
    if is_ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
