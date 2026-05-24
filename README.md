# llm-fallback-router

[![Crates.io](https://img.shields.io/crates/v/llm-fallback-router.svg)](https://crates.io/crates/llm-fallback-router)
[![Documentation](https://docs.rs/llm-fallback-router/badge.svg)](https://docs.rs/llm-fallback-router)
[![License](https://img.shields.io/crates/l/llm-fallback-router.svg)](https://crates.io/crates/llm-fallback-router)

**Multi-provider failover for LLM calls.** Try Anthropic, fall back to OpenAI or
Gemini or Bedrock on retryable errors. Per-attempt audit log. Zero runtime
deps. BYO clients.

```rust
use llm_fallback_router::{Router, Provider, RetryHint};

#[derive(Debug)]
enum MyError { RateLimit, BadAuth }

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MyError {}
impl RetryHint for MyError {
    fn is_retryable(&self) -> bool { matches!(self, MyError::RateLimit) }
}

fn anthropic_call(req: &str) -> Result<String, MyError> { Err(MyError::RateLimit) }
fn openai_call(req: &str) -> Result<String, MyError> { Ok(req.to_string()) }

let router = Router::new(vec![
    Provider::new("anthropic", anthropic_call),
    Provider::new("openai",    openai_call),
]).unwrap();

let result = router.complete(&"hi".to_string()).unwrap();
println!("{} {}", result.provider, result.tries);
// "openai" 2 on failover
```

## Why

In production, the question is never "what's my best provider", it's "what do
I do when my best provider is having a bad ten minutes." Half the
LLM-resilience guides on the internet are slow tutorial code that does this
in 30 lines and forgets to record what happened.

`llm-fallback-router` is the small version that:

- Tries providers in order, falls through on retryable errors only
- Defaults to a status-code list that covers Anthropic / OpenAI / Google / Bedrock
- Lets you supply your own retryable predicate (per-provider or global)
- Calls `on_attempt(&Attempt)` for every try so you can audit and meter
- Stays out of your way on request shape — you decide what the request type
  looks like, the router stays parametric over `Req`, `Resp`, `E`

Sibling to the Python crate
[`llm-fallback-router`](https://github.com/MukundaKatta/llm-fallback-router).

## Install

```toml
[dependencies]
llm-fallback-router = "0.1"
```

## API

```rust
use llm_fallback_router::{Provider, Router};

let provider = Provider::new("anthropic", |req: &MyReq| anthropic_call(req))
    .with_retry_predicate(|err: &MyError| /* ... */ true);

let router = Router::new(vec![provider, /* ... */])
    .unwrap()
    .with_retry_predicate(|err: &MyError| /* global override */ true)
    .with_on_attempt(|attempt| println!("{}: ok={}", attempt.provider, attempt.ok));

let result = router.complete(&request)?;
```

`RouteResult<Resp>` has `.provider` (winner), `.response` (the raw provider
reply), `.tries`, and `.attempts: Vec<Attempt>` with per-attempt latency and
error info.

### `RetryHint` vs closures

Rust does not have Python-style runtime exception class inspection. To keep
the default ergonomic, this crate exposes a `RetryHint` trait:

```rust
use llm_fallback_router::{
    RetryHint,
    default_is_retryable_status,
    default_is_retryable_by_name,
};

impl RetryHint for MyError {
    fn is_retryable(&self) -> bool {
        match self {
            MyError::Http(s) => default_is_retryable_status(*s),
            MyError::Named(n) => default_is_retryable_by_name(n),
            MyError::BadAuth => false,
        }
    }
}
```

If your error type cannot implement `RetryHint` (e.g. it's a foreign type),
construct the router with an explicit predicate instead:

```rust
let router = Router::with_providers_and_predicate(
    providers,
    |err: &SomeForeignError| /* ... */ true,
)?;
```

Per-provider predicates always override the router's global predicate for
their provider.

### `default_is_retryable_status` covers

`408`, `409`, `425`, `429`, `500`, `502`, `503`, `504`, `529`.

### `default_is_retryable_by_name` covers

Class-name keywords: `RateLimit`, `ServiceUnavailable`, `Overloaded`,
`Timeout`, `APIConnectionError`, `InternalServer`, `ThrottlingException`,
`ModelStreamErrorException`, `ServiceQuotaExceeded`.

## Errors

```rust
pub enum RouteError<E> {
    /// A provider failed with a non-retryable error. The original error is
    /// surfaced unchanged.
    NonRetryable(E),
    /// Every provider in the chain failed with a retryable error. The full
    /// per-attempt audit log is included.
    AllProvidersFailed(Vec<Attempt>),
}
```

A 401 from Anthropic surfaces as `RouteError::NonRetryable` rather than
silently draining budget on three other providers.

## What it does NOT do

- No HTTP client. This crate doesn't talk to any LLM provider.
- No backoff between attempts; if you want retry of a *single* provider,
  pair this with a retry helper.
- No async runtime. The provider call is a synchronous `Fn(&Req) -> Result`.
  Wrap in your own async executor as needed.

## License

MIT OR Apache-2.0
