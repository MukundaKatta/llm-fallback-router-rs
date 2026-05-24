//! Integration tests mirroring the Python sibling.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use llm_fallback_router::{
    default_is_retryable, default_is_retryable_by_name, default_is_retryable_status, Attempt,
    Provider, RetryHint, RouteError, Router,
};

// ---- fake error type --------------------------------------------------------

#[derive(Debug, Clone)]
enum FakeError {
    RateLimit,
    Overloaded,
    AuthError,
    HttpStatus(u16),
    /// Used to verify class-name keyword fallback.
    ServiceUnavailable,
}

impl std::fmt::Display for FakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FakeError::RateLimit => write!(f, "rate limited"),
            FakeError::Overloaded => write!(f, "overloaded"),
            FakeError::AuthError => write!(f, "bad auth"),
            FakeError::HttpStatus(s) => write!(f, "http {s}"),
            FakeError::ServiceUnavailable => write!(f, "service unavailable"),
        }
    }
}

impl std::error::Error for FakeError {}

impl RetryHint for FakeError {
    fn is_retryable(&self) -> bool {
        match self {
            FakeError::RateLimit => {
                default_is_retryable_by_name("RateLimitError")
                    || default_is_retryable_status(429)
            }
            FakeError::Overloaded => default_is_retryable_by_name("OverloadedError"),
            FakeError::ServiceUnavailable => {
                default_is_retryable_by_name("ServiceUnavailableError")
            }
            FakeError::HttpStatus(s) => default_is_retryable_status(*s),
            FakeError::AuthError => false,
        }
    }
}

// ---- happy path -------------------------------------------------------------

#[test]
fn first_provider_wins_returns_route_result() {
    let router = Router::<&str, String, FakeError>::new(vec![
        Provider::new("primary", |req: &&str| Ok(format!("hello {req}"))),
        Provider::new("secondary", |_req: &&str| {
            panic!("must not be called")
        }),
    ])
    .unwrap();

    let out = router.complete(&"world").unwrap();
    assert_eq!(out.provider, "primary");
    assert_eq!(out.tries, 1);
    assert_eq!(out.response, "hello world");
    assert_eq!(out.attempts.len(), 1);
    assert!(out.attempts[0].ok);
}

#[test]
fn falls_through_on_retryable_error() {
    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("primary", |_| Err(FakeError::RateLimit)),
        Provider::new("secondary", |_| Ok("ok")),
    ])
    .unwrap();

    let out = router.complete(&()).unwrap();
    assert_eq!(out.provider, "secondary");
    assert_eq!(out.tries, 2);
    assert!(!out.attempts[0].ok);
    assert_eq!(out.attempts[0].error_type.as_deref(), Some("FakeError"));
    assert_eq!(out.attempts[0].error_message.as_deref(), Some("rate limited"));
    assert!(out.attempts[1].ok);
}

#[test]
fn falls_through_multiple_times() {
    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("a", |_| Err(FakeError::RateLimit)),
        Provider::new("b", |_| Err(FakeError::Overloaded)),
        Provider::new("c", |_| Ok("win")),
    ])
    .unwrap();

    let out = router.complete(&()).unwrap();
    assert_eq!(out.provider, "c");
    assert_eq!(out.tries, 3);
}

// ---- non-retryable behavior -------------------------------------------------

#[test]
fn non_retryable_surfaces_immediately() {
    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("a", |_| Err(FakeError::AuthError)),
        Provider::new("b", |_| Ok("should not see this")),
    ])
    .unwrap();

    match router.complete(&()) {
        Err(RouteError::NonRetryable(FakeError::AuthError)) => {}
        other => panic!("expected NonRetryable(AuthError), got {other:?}"),
    }
}

#[test]
fn all_fail_returns_all_providers_failed() {
    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("a", |_| Err(FakeError::RateLimit)),
        Provider::new("b", |_| Err(FakeError::Overloaded)),
    ])
    .unwrap();

    match router.complete(&()) {
        Err(RouteError::AllProvidersFailed(attempts)) => {
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[0].provider, "a");
            assert_eq!(attempts[1].provider, "b");
            assert!(!attempts[0].ok);
            assert!(!attempts[1].ok);
            assert_eq!(attempts[0].error_message.as_deref(), Some("rate limited"));
            assert_eq!(attempts[1].error_message.as_deref(), Some("overloaded"));
        }
        other => panic!("expected AllProvidersFailed, got {other:?}"),
    }
}

// ---- default predicate ------------------------------------------------------

#[test]
fn default_is_retryable_status_hits_each_documented_code() {
    for status in [408_u16, 409, 425, 429, 500, 502, 503, 504, 529] {
        assert!(
            default_is_retryable_status(status),
            "status {status} should be retryable"
        );
    }
    // a non-retryable status for control
    assert!(!default_is_retryable_status(401));
    assert!(!default_is_retryable_status(404));
}

#[test]
fn default_is_retryable_by_name_hits_each_keyword() {
    for kw in [
        "RateLimit",
        "ServiceUnavailable",
        "Overloaded",
        "Timeout",
        "APIConnectionError",
        "InternalServer",
        "ThrottlingException",
        "ModelStreamErrorException",
        "ServiceQuotaExceeded",
    ] {
        let full = format!("Vendor{kw}Error");
        assert!(
            default_is_retryable_by_name(&full),
            "name {full} should match"
        );
    }
    assert!(!default_is_retryable_by_name("AuthError"));
    assert!(!default_is_retryable_by_name("BadRequest"));
}

#[test]
fn default_is_retryable_dispatches_through_trait() {
    assert!(default_is_retryable(&FakeError::RateLimit));
    assert!(default_is_retryable(&FakeError::Overloaded));
    assert!(default_is_retryable(&FakeError::ServiceUnavailable));
    assert!(default_is_retryable(&FakeError::HttpStatus(503)));
    assert!(!default_is_retryable(&FakeError::AuthError));
    assert!(!default_is_retryable(&FakeError::HttpStatus(401)));
}

// ---- custom predicates ------------------------------------------------------

#[test]
fn per_provider_predicate_overrides_global() {
    let router = Router::<(), &'static str, FakeError>::new(vec![
        // AuthError is normally non-retryable, but this provider says "always retry"
        Provider::new("a", |_| Err(FakeError::AuthError))
            .with_retry_predicate(|_e| true),
        Provider::new("b", |_| Ok("second")),
    ])
    .unwrap();

    let out = router.complete(&()).unwrap();
    assert_eq!(out.provider, "b");
}

#[test]
fn global_predicate_overrides_default() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = calls.clone();

    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("a", |_| Err(FakeError::RateLimit)),
        Provider::new("b", |_| Ok("second")),
    ])
    .unwrap()
    .with_retry_predicate(move |err: &FakeError| {
        recorded.lock().unwrap().push(format!("{err:?}"));
        false
    });

    match router.complete(&()) {
        Err(RouteError::NonRetryable(FakeError::RateLimit)) => {}
        other => panic!("expected NonRetryable(RateLimit), got {other:?}"),
    }
    assert_eq!(calls.lock().unwrap().len(), 1);
}

// ---- on_attempt audit hook --------------------------------------------------

#[test]
fn on_attempt_called_for_both_success_and_failure() {
    let seen: Arc<Mutex<Vec<Attempt>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();

    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("a", |_| Err(FakeError::RateLimit)),
        Provider::new("b", |_| Ok("ok")),
    ])
    .unwrap()
    .with_on_attempt(move |a: &Attempt| {
        captured.lock().unwrap().push(a.clone());
    });

    router.complete(&()).unwrap();

    let seen = seen.lock().unwrap();
    let names: Vec<&str> = seen.iter().map(|a| a.provider.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
    assert!(!seen[0].ok);
    assert!(seen[1].ok);
}

// ---- misc -------------------------------------------------------------------

#[test]
fn empty_provider_list_is_rejected() {
    let providers: Vec<Provider<(), &'static str, FakeError>> = Vec::new();
    assert!(Router::new(providers).is_err());
}

#[test]
fn providers_accessor_returns_borrowed_slice() {
    let router = Router::<(), &'static str, FakeError>::new(vec![Provider::new("a", |_| {
        Ok("x")
    })])
    .unwrap();
    assert_eq!(router.providers().len(), 1);
    assert_eq!(router.providers()[0].name, "a");
}

#[test]
fn attempt_records_latency_for_slow_handler() {
    let router = Router::<(), &'static str, FakeError>::new(vec![Provider::new("slow", |_| {
        thread::sleep(Duration::from_millis(15));
        Ok("ok")
    })])
    .unwrap();
    let out = router.complete(&()).unwrap();
    assert!(
        out.attempts[0].latency_ms >= 10,
        "expected >=10ms latency, got {}",
        out.attempts[0].latency_ms
    );
}

// ---- without RetryHint (closure-only) ---------------------------------------

#[test]
fn router_without_retry_hint_uses_explicit_predicate() {
    // Plain string error, no RetryHint impl: build via the explicit-predicate
    // constructor.
    let providers: Vec<Provider<(), &'static str, String>> = vec![
        Provider::new("a", |_| Err("rate-limit-x".to_string())),
        Provider::new("b", |_| Ok("won")),
    ];
    let router = Router::with_providers_and_predicate(providers, |e: &String| {
        e.contains("rate-limit")
    })
    .unwrap();

    let out = router.complete(&()).unwrap();
    assert_eq!(out.provider, "b");
    assert_eq!(out.tries, 2);
}

// ---- ordering / call count under fall-through -------------------------------

#[test]
fn second_provider_only_called_after_first_fails() {
    let primary_calls = Arc::new(AtomicUsize::new(0));
    let secondary_calls = Arc::new(AtomicUsize::new(0));
    let p1 = primary_calls.clone();
    let p2 = secondary_calls.clone();

    let router = Router::<(), &'static str, FakeError>::new(vec![
        Provider::new("primary", move |_| {
            p1.fetch_add(1, Ordering::SeqCst);
            Ok("first")
        }),
        Provider::new("secondary", move |_| {
            p2.fetch_add(1, Ordering::SeqCst);
            Ok("second")
        }),
    ])
    .unwrap();

    router.complete(&()).unwrap();
    assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_calls.load(Ordering::SeqCst), 0);
}

// ---- RouteError Display / Error -------------------------------------------

#[test]
fn route_error_display_and_error_impls() {
    let attempts = vec![
        Attempt {
            provider: "a".into(),
            ok: false,
            latency_ms: 1,
            error_type: Some("FakeError".into()),
            error_message: Some("x".into()),
        },
        Attempt {
            provider: "b".into(),
            ok: false,
            latency_ms: 1,
            error_type: Some("FakeError".into()),
            error_message: Some("y".into()),
        },
    ];
    let err: RouteError<FakeError> = RouteError::AllProvidersFailed(attempts);
    let s = format!("{err}");
    assert!(s.contains("a(FakeError)"));
    assert!(s.contains("b(FakeError)"));

    let nr: RouteError<FakeError> = RouteError::NonRetryable(FakeError::AuthError);
    let s = format!("{nr}");
    assert!(s.contains("bad auth"));

    // confirms std::error::Error is implemented
    fn _take_error<E: std::error::Error>(_: &E) {}
    _take_error(&nr);
}
