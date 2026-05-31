pub mod x;
pub mod x_app;
pub mod oauth;

/// The crate's canonical error type — the X v2 HTTP client's
/// [`x::Error`] re-exported so call sites can write
/// `crate::Error` regardless of which module surfaces the
/// failure (HTTP, token endpoint, file IO).
pub use x::Error;
