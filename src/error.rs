use std::fmt;

/// Errors returned by proxy resolution and PAC script execution.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No proxy resolver is available on this system.
    NoResolver,
    /// No PAC script engine is available.
    NoScriptEngine,
    /// The PAC script could not be evaluated.
    Script(String),
    /// The PAC script could not be downloaded.
    Fetch(String),
    /// The operating system returned an error code.
    Os(i32),
    /// The operation was cancelled.
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoResolver => write!(f, "no proxy resolver available"),
            Error::NoScriptEngine => write!(f, "no PAC script engine available"),
            Error::Script(message) => write!(f, "PAC script error: {message}"),
            Error::Fetch(message) => write!(f, "unable to fetch PAC script: {message}"),
            Error::Os(code) => write!(f, "system error {code}"),
            Error::Cancelled => write!(f, "operation cancelled"),
        }
    }
}

impl std::error::Error for Error {}
