//! Default retryable predicate.
//!
//! Rust does not have Python's runtime exception class hierarchy, so the
//! default predicate dispatches through a small [`RetryHint`] trait that
//! user-defined error types implement. Status-code and class-name helpers
//! are exposed standalone so a custom impl (or a closure) can reuse them.

/// Hint a user error type gives the router about whether it should be
/// retried.
///
/// Implement this on your error type to opt in to [`default_is_retryable`].
/// If you'd rather not, supply a closure with
/// [`crate::Router::with_retry_predicate`] instead.
pub trait RetryHint {
    /// Return `true` if this error should cause the router to fall through
    /// to the next provider, `false` if it should be surfaced immediately.
    fn is_retryable(&self) -> bool;
}

/// Status codes that are considered retryable by default.
///
/// Matches the Python sibling: 408, 409, 425, 429, 500, 502, 503, 504, 529.
pub fn default_is_retryable_status(status: u16) -> bool {
    matches!(
        status,
        408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529
    )
}

/// Class-name keyword check used by the default retryable predicate.
///
/// Returns `true` if `name` contains any of the vendor-specific keywords
/// covered by Anthropic / OpenAI / Google / Bedrock conventions:
/// `RateLimit`, `ServiceUnavailable`, `Overloaded`, `Timeout`,
/// `APIConnectionError`, `InternalServer`, `ThrottlingException`,
/// `ModelStreamErrorException`, `ServiceQuotaExceeded`.
pub fn default_is_retryable_by_name(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "RateLimit",
        "ServiceUnavailable",
        "Overloaded",
        "Timeout",
        "APIConnectionError",
        "InternalServer",
        "ThrottlingException",
        "ModelStreamErrorException",
        "ServiceQuotaExceeded",
    ];
    KEYWORDS.iter().any(|kw| name.contains(kw))
}

/// Reasonable default retryable predicate.
///
/// Delegates entirely to `E`'s [`RetryHint`] impl. If you want the
/// status-code or class-name keyword behavior of the Python sibling, call
/// [`default_is_retryable_status`] / [`default_is_retryable_by_name`] from
/// inside your [`RetryHint`] impl.
pub fn default_is_retryable<E: RetryHint>(err: &E) -> bool {
    err.is_retryable()
}
