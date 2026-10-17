//! Command line utility for reading proxy configuration and resolving proxies.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use proxyres::resolver::Resolver;
use proxyres::{config, execute};

/// Read proxy configuration, execute PAC scripts, and resolve proxies for urls.
#[derive(Parser)]
#[command(name = "proxycli", bin_name = "proxycli", version, arg_required_else_help = true)]
struct Cli {
    /// Print progress and debug logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Dump proxy configuration values
    Config {
        /// Value to print, or all values if not specified
        option: Option<ConfigOption>,
        /// Scheme of the proxy to print
        #[arg(default_value = "http")]
        scheme: String,
    },
    /// Execute a PAC script for urls
    Execute {
        /// PAC script file
        file: PathBuf,
        /// Urls to pass to FindProxyForURL
        #[arg(required = true)]
        urls: Vec<String>,
    },
    /// Resolve proxies for urls
    Resolve {
        /// Urls to resolve
        #[arg(required = true)]
        urls: Vec<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
enum ConfigOption {
    AutoDiscover,
    AutoConfigUrl,
    Proxy,
    BypassList,
}

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

fn print_proxy_config(option: Option<ConfigOption>, scheme: &str) {
    let all = option.is_none();
    if all {
        println!("Proxy configuration");
    }
    let label = |name: &str| if all { format!("  {name}: ") } else { String::new() };
    let show = |shown: ConfigOption| all || option == Some(shown);

    if show(ConfigOption::AutoDiscover) {
        let auto_discover = if config::auto_discover() { "enabled" } else { "disabled" };
        println!("{}{auto_discover}", label("Auto discover"));
    }
    if show(ConfigOption::AutoConfigUrl) {
        println!("{}{}", label("Auto config url"), or_not_set(config::auto_config_url()));
    }
    if show(ConfigOption::Proxy) {
        println!("{}{}", label("Proxy"), or_not_set(config::proxy(scheme)));
    }
    if show(ConfigOption::BypassList) {
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

fn execute_pac_script(script_path: &Path, urls: &[String], verbose: bool) -> bool {
    let script = match std::fs::read_to_string(script_path) {
        Ok(script) => script,
        Err(error) => {
            println!("Failed to read PAC script {} ({error})", script_path.display());
            return false;
        }
    };
    let mut is_ok = true;
    for url in urls {
        if verbose {
            println!("Executing PAC script {} for {url}", script_path.display());
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

fn main() -> ExitCode {
    let cli = Cli::parse();

    let _ = log::set_logger(&StderrLogger);
    log::set_max_level(if cli.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Error
    });

    let is_ok = match &cli.command {
        Command::Config { option, scheme } => {
            print_proxy_config(*option, scheme);
            true
        }
        Command::Execute { file, urls } => execute_pac_script(file, urls, cli.verbose),
        Command::Resolve { urls } => resolve_proxies_for_urls(urls, cli.verbose),
    };
    if is_ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
