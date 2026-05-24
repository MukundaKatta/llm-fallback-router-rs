//! # llm-fallback-router
//!
//! Multi-provider failover for LLM calls.
//!
//! You want Anthropic. You also don't want a 503 from Anthropic to break a
//! production agent. [`Router`] takes an ordered list of providers and falls
//! through on retryable errors.
//!
//! ```no_run
//! use llm_fallback_router::{Router, Provider, RetryHint};
//!
//! #[derive(Debug)]
//! enum MyError {
//!     RateLimit,
//!     BadAuth,
//! }
//!
//! impl std::fmt::Display for MyError {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         write!(f, "{self:?}")
//!     }
//! }
//! impl std::error::Error for MyError {}
//! impl RetryHint for MyError {
//!     fn is_retryable(&self) -> bool {
//!         matches!(self, MyError::RateLimit)
//!     }
//! }
//!
//! fn anthropic_call(_req: &String) -> Result<String, MyError> { Err(MyError::RateLimit) }
//! fn openai_call(req: &String) -> Result<String, MyError> { Ok(format!("ok: {req}")) }
//!
//! let router = Router::<String, String, MyError>::new(vec![
//!     Provider::new("anthropic", anthropic_call),
//!     Provider::new("openai", openai_call),
//! ]).unwrap();
//!
//! let result = router.complete(&"hi".to_string()).unwrap();
//! assert_eq!(result.provider, "openai");
//! assert_eq!(result.tries, 2);
//! ```
//!
//! ## Design: trait vs closure for `is_retryable`
//!
//! Rust does not have Python-style runtime exception class inspection. To
//! keep `default_is_retryable` ergonomic without forcing every caller to
//! plug in a closure, this crate exposes a [`RetryHint`] trait that user
//! error types implement. [`default_is_retryable`] dispatches through it.
//!
//! If you'd rather not implement a trait (e.g. you're wrapping a third-party
//! error type), supply a closure via [`Router::with_retry_predicate`] or
//! [`Provider::with_retry_predicate`]; the closure wins over the default.
//!
//! [`default_is_retryable_status`] and [`default_is_retryable_by_name`] are
//! exposed as standalone helpers so a custom [`RetryHint`] impl (or a
//! closure) can reuse the same status-code list and class-name keywords as
//! the Python sibling.

#![deny(missing_docs)]

mod attempt;
mod retryable;
mod router;

pub use attempt::{Attempt, RouteResult};
pub use retryable::{
    default_is_retryable, default_is_retryable_by_name, default_is_retryable_status, RetryHint,
};
pub use router::{Provider, RouteError, Router};
