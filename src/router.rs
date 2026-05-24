//! [`Router`] and [`Provider`] core.

use std::fmt;
use std::time::Instant;

use crate::attempt::{Attempt, RouteResult};
use crate::retryable::{default_is_retryable, RetryHint};

/// Closure type for calling a provider.
type CallFn<Req, Resp, E> = Box<dyn Fn(&Req) -> Result<Resp, E> + Send + Sync>;

/// Closure type for a retryable predicate.
type RetryableFn<E> = Box<dyn Fn(&E) -> bool + Send + Sync>;

/// Closure type for an on-attempt audit callback.
type OnAttemptFn = Box<dyn Fn(&Attempt) + Send + Sync>;

/// A single provider in the fallback chain.
pub struct Provider<Req, Resp, E> {
    /// Provider name, surfaced in [`Attempt::provider`] and
    /// [`RouteResult::provider`].
    pub name: String,
    /// Callable that performs the provider's request.
    pub call: CallFn<Req, Resp, E>,
    /// Optional per-provider retryable predicate. When set, it wins over
    /// the router's global predicate for this provider.
    pub is_retryable: Option<RetryableFn<E>>,
}

impl<Req, Resp, E> Provider<Req, Resp, E> {
    /// Construct a provider with a name and call function.
    pub fn new<S, F>(name: S, call: F) -> Self
    where
        S: Into<String>,
        F: Fn(&Req) -> Result<Resp, E> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            call: Box::new(call),
            is_retryable: None,
        }
    }

    /// Attach a per-provider retryable predicate that overrides the
    /// router's global predicate for this provider only.
    pub fn with_retry_predicate<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        self.is_retryable = Some(Box::new(predicate));
        self
    }
}

impl<Req, Resp, E> fmt::Debug for Provider<Req, Resp, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .field("call", &"<fn>")
            .field("is_retryable", &self.is_retryable.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

/// Error returned by [`Router::complete`].
#[derive(Debug)]
pub enum RouteError<E> {
    /// A provider failed with an error the retryable predicate rejected.
    /// The original error is surfaced unchanged so callers can inspect it.
    NonRetryable(E),
    /// Every provider in the chain failed with a retryable error. The
    /// per-attempt audit log is included for inspection.
    AllProvidersFailed(Vec<Attempt>),
}

impl<E: fmt::Display> fmt::Display for RouteError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::NonRetryable(e) => write!(f, "non-retryable provider error: {e}"),
            RouteError::AllProvidersFailed(attempts) => {
                let names: Vec<String> = attempts
                    .iter()
                    .map(|a| {
                        format!(
                            "{}({})",
                            a.provider,
                            a.error_type.as_deref().unwrap_or("?")
                        )
                    })
                    .collect();
                write!(f, "all providers failed: {}", names.join(", "))
            }
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RouteError<E> {}

/// Try providers in order, fall through on retryable errors.
///
/// The retryable predicate hierarchy is: per-provider closure (if set)
/// wins over the router's global predicate, which wins over the default
/// (which dispatches through [`RetryHint`]).
pub struct Router<Req, Resp, E> {
    providers: Vec<Provider<Req, Resp, E>>,
    is_retryable: RetryableFn<E>,
    on_attempt: Option<OnAttemptFn>,
}

impl<Req, Resp, E> Router<Req, Resp, E>
where
    E: RetryHint + fmt::Display,
{
    /// Build a router from an ordered list of providers, using
    /// [`default_is_retryable`] as the global predicate.
    ///
    /// Returns `Err` with the original vec if `providers` is empty.
    pub fn new(providers: Vec<Provider<Req, Resp, E>>) -> Result<Self, Vec<Provider<Req, Resp, E>>>
    {
        if providers.is_empty() {
            return Err(providers);
        }
        Ok(Self {
            providers,
            is_retryable: Box::new(|e: &E| default_is_retryable(e)),
            on_attempt: None,
        })
    }
}

impl<Req, Resp, E> Router<Req, Resp, E>
where
    E: fmt::Display,
{
    /// Build a router with an explicit global retryable predicate. Use
    /// this when your error type cannot implement [`RetryHint`].
    ///
    /// Returns `Err` with the original vec if `providers` is empty.
    pub fn with_providers_and_predicate<F>(
        providers: Vec<Provider<Req, Resp, E>>,
        predicate: F,
    ) -> Result<Self, Vec<Provider<Req, Resp, E>>>
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        if providers.is_empty() {
            return Err(providers);
        }
        Ok(Self {
            providers,
            is_retryable: Box::new(predicate),
            on_attempt: None,
        })
    }

    /// Replace the router's global retryable predicate. Per-provider
    /// predicates still override this one for their own provider.
    pub fn with_retry_predicate<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        self.is_retryable = Box::new(predicate);
        self
    }

    /// Attach a callback that fires after every attempt, success or
    /// failure. Useful for audit logs and metrics.
    pub fn with_on_attempt<F>(mut self, callback: F) -> Self
    where
        F: Fn(&Attempt) + Send + Sync + 'static,
    {
        self.on_attempt = Some(Box::new(callback));
        self
    }

    /// Borrow the provider list. The router owns its providers; this is a
    /// non-mutating accessor.
    pub fn providers(&self) -> &[Provider<Req, Resp, E>] {
        &self.providers
    }

    /// Send `request` through the chain until one provider succeeds.
    ///
    /// Returns [`RouteError::NonRetryable`] with the original error on the
    /// first non-retryable failure. Returns
    /// [`RouteError::AllProvidersFailed`] if every provider failed with a
    /// retryable error.
    pub fn complete(&self, request: &Req) -> Result<RouteResult<Resp>, RouteError<E>> {
        let mut attempts: Vec<Attempt> = Vec::with_capacity(self.providers.len());

        for (i, provider) in self.providers.iter().enumerate() {
            let started = Instant::now();
            match (provider.call)(request) {
                Ok(response) => {
                    let attempt = Attempt {
                        provider: provider.name.clone(),
                        ok: true,
                        latency_ms: started.elapsed().as_millis() as u64,
                        error_type: None,
                        error_message: None,
                    };
                    attempts.push(attempt.clone());
                    if let Some(cb) = &self.on_attempt {
                        cb(&attempt);
                    }
                    return Ok(RouteResult {
                        provider: provider.name.clone(),
                        response,
                        tries: i + 1,
                        attempts,
                    });
                }
                Err(err) => {
                    let mut msg = format!("{err}");
                    if msg.len() > 512 {
                        // truncate on char boundary
                        let end = msg
                            .char_indices()
                            .take_while(|(idx, _)| *idx < 512)
                            .last()
                            .map(|(idx, ch)| idx + ch.len_utf8())
                            .unwrap_or(0);
                        msg.truncate(end);
                    }
                    let attempt = Attempt {
                        provider: provider.name.clone(),
                        ok: false,
                        latency_ms: started.elapsed().as_millis() as u64,
                        error_type: Some(short_type_name::<E>().to_string()),
                        error_message: Some(msg),
                    };
                    attempts.push(attempt.clone());
                    if let Some(cb) = &self.on_attempt {
                        cb(&attempt);
                    }

                    let retryable = match &provider.is_retryable {
                        Some(p) => p(&err),
                        None => (self.is_retryable)(&err),
                    };
                    if !retryable {
                        return Err(RouteError::NonRetryable(err));
                    }
                    // retryable: try the next provider
                }
            }
        }

        Err(RouteError::AllProvidersFailed(attempts))
    }
}

/// Return a short type name for `T`. Strips module path so we get
/// `RateLimitError` instead of `crate::module::RateLimitError`.
fn short_type_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}
