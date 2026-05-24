//! Result and per-attempt audit types.

/// Record of a single provider call, success or failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// Provider name.
    pub provider: String,
    /// Whether the call succeeded.
    pub ok: bool,
    /// Wall-clock latency of the attempt, milliseconds.
    pub latency_ms: u64,
    /// Short type name of the error, when the attempt failed.
    pub error_type: Option<String>,
    /// First 512 chars of the error's `Display` output.
    pub error_message: Option<String>,
}

/// Result of a successful [`crate::Router::complete`] call.
#[derive(Debug, Clone)]
pub struct RouteResult<Resp> {
    /// Name of the provider that produced the response.
    pub provider: String,
    /// The response returned by that provider.
    pub response: Resp,
    /// Number of providers tried (1-indexed; equals winning provider's
    /// position in the chain).
    pub tries: usize,
    /// Per-attempt audit log, including the final winning attempt.
    pub attempts: Vec<Attempt>,
}
