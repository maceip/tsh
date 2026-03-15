// ==========================================================================
// Gateway Service — Request routing, rate limiting, and payment processing
// ==========================================================================
//
// This module handles incoming API requests, applies rate limiting per tenant,
// routes to appropriate backend services, and processes payment transactions.
//
// Architecture:
//   Client -> Gateway -> RateLimiter -> Router -> Backend
//                                    -> PaymentProcessor -> Stripe API

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub listen_addr: String,
    pub listen_port: u16,
    pub max_connections: usize,
    pub request_timeout: Duration,
    pub idle_timeout: Duration,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
    pub backend_urls: Vec<String>,
    pub health_check_interval: Duration,
    pub log_level: LogLevel,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 8080,
            max_connections: 10_000,
            request_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
            tls_cert_path: None,
            tls_key_path: None,
            backend_urls: vec!["http://localhost:3000".to_string()],
            health_check_interval: Duration::from_secs(10),
            log_level: LogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub requests_per_second: u32,
    pub burst_size: u32,
    pub window_size: Duration,
    pub penalty_duration: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 100,
            burst_size: 200,
            window_size: Duration::from_secs(60),
            penalty_duration: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(pub String);

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self(format!("req_{:x}", timestamp))
    }
}

#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub id: RequestId,
    pub tenant_id: TenantId,
    pub method: HttpMethod,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timestamp: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
            HttpMethod::Patch => write!(f, "PATCH"),
            HttpMethod::Head => write!(f, "HEAD"),
            HttpMethod::Options => write!(f, "OPTIONS"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub latency: Duration,
}

impl ApiResponse {
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            headers: HashMap::new(),
            body,
            latency: Duration::default(),
        }
    }

    pub fn error(status_code: u16, message: &str) -> Self {
        Self {
            status_code,
            headers: HashMap::new(),
            body: format!("{{\"error\": \"{}\"}}", message).into_bytes(),
            latency: Duration::default(),
        }
    }

    pub fn rate_limited() -> Self {
        Self::error(429, "Rate limit exceeded")
    }

    pub fn not_found() -> Self {
        Self::error(404, "Not found")
    }

    pub fn internal_error() -> Self {
        Self::error(500, "Internal server error")
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum GatewayError {
    RateLimited { tenant_id: TenantId, retry_after: Duration },
    BackendUnavailable { backend_url: String },
    Timeout { elapsed: Duration },
    InvalidRequest { message: String },
    AuthenticationFailed { reason: String },
    InternalError { source: Box<dyn std::error::Error + Send + Sync> },
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayError::RateLimited { tenant_id, retry_after } => {
                write!(f, "Rate limited for tenant {}: retry after {:?}", tenant_id, retry_after)
            }
            GatewayError::BackendUnavailable { backend_url } => {
                write!(f, "Backend unavailable: {}", backend_url)
            }
            GatewayError::Timeout { elapsed } => {
                write!(f, "Request timed out after {:?}", elapsed)
            }
            GatewayError::InvalidRequest { message } => {
                write!(f, "Invalid request: {}", message)
            }
            GatewayError::AuthenticationFailed { reason } => {
                write!(f, "Authentication failed: {}", reason)
            }
            GatewayError::InternalError { source } => {
                write!(f, "Internal error: {}", source)
            }
        }
    }
}

impl std::error::Error for GatewayError {}

// ---------------------------------------------------------------------------
// Rate Limiter — Token Bucket Algorithm
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self, tokens: f64) -> bool {
        self.refill();
        if self.tokens >= tokens {
            self.tokens -= tokens;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }
}

pub struct RateLimiter {
    config: RateLimitConfig,
    buckets: Mutex<HashMap<TenantId, TokenBucket>>,
    penalties: Mutex<HashMap<TenantId, Instant>>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Mutex::new(HashMap::new()),
            penalties: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, tenant_id: &TenantId) -> Result<(), GatewayError> {
        // Check for active penalties
        {
            let penalties = self.penalties.lock().unwrap();
            if let Some(penalty_until) = penalties.get(tenant_id) {
                if Instant::now() < *penalty_until {
                    return Err(GatewayError::RateLimited {
                        tenant_id: tenant_id.clone(),
                        retry_after: penalty_until.duration_since(Instant::now()),
                    });
                }
            }
        }

        // Check token bucket
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets
            .entry(tenant_id.clone())
            .or_insert_with(|| {
                TokenBucket::new(
                    self.config.burst_size as f64,
                    self.config.requests_per_second as f64,
                )
            });

        if bucket.try_consume(1.0) {
            Ok(())
        } else {
            // Apply penalty for exceeding rate limit
            let mut penalties = self.penalties.lock().unwrap();
            penalties.insert(
                tenant_id.clone(),
                Instant::now() + self.config.penalty_duration,
            );
            Err(GatewayError::RateLimited {
                tenant_id: tenant_id.clone(),
                retry_after: self.config.penalty_duration,
            })
        }
    }

    pub fn reset(&self, tenant_id: &TenantId) {
        let mut buckets = self.buckets.lock().unwrap();
        buckets.remove(tenant_id);
        let mut penalties = self.penalties.lock().unwrap();
        penalties.remove(tenant_id);
    }
}

// ---------------------------------------------------------------------------
// Backend health checking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct BackendState {
    pub url: String,
    pub status: BackendStatus,
    pub last_check: Instant,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub total_errors: u64,
    pub avg_latency_ms: f64,
}

impl BackendState {
    pub fn new(url: String) -> Self {
        Self {
            url,
            status: BackendStatus::Healthy,
            last_check: Instant::now(),
            consecutive_failures: 0,
            total_requests: 0,
            total_errors: 0,
            avg_latency_ms: 0.0,
        }
    }

    pub fn record_success(&mut self, latency: Duration) {
        self.consecutive_failures = 0;
        self.total_requests += 1;
        let latency_ms = latency.as_secs_f64() * 1000.0;
        self.avg_latency_ms = (self.avg_latency_ms * (self.total_requests - 1) as f64
            + latency_ms)
            / self.total_requests as f64;
        self.status = if self.avg_latency_ms > 1000.0 {
            BackendStatus::Degraded
        } else {
            BackendStatus::Healthy
        };
    }

    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.total_requests += 1;
        self.total_errors += 1;
        self.status = if self.consecutive_failures >= 3 {
            BackendStatus::Unhealthy
        } else {
            BackendStatus::Degraded
        };
    }

    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_errors as f64 / self.total_requests as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Memory buffer — raw pointer manipulation for zero-copy parsing
// ---------------------------------------------------------------------------

pub struct RawBuffer {
    ptr: *mut u8,
    len: usize,
    capacity: usize,
}

impl RawBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(capacity, 8).unwrap();
        // SAFETY: this is actually unsound, see issue #1234
        // We're allocating raw memory and casting to *mut u8 but we never
        // properly track the allocator layout for deallocation, and the
        // Drop impl uses a potentially different layout.
        let ptr = unsafe { std::alloc::alloc(layout) };
        Self {
            ptr,
            len: 0,
            capacity,
        }
    }

    pub fn push(&mut self, byte: u8) {
        if self.len >= self.capacity {
            self.grow();
        }
        unsafe {
            *self.ptr.add(self.len) = byte;
        }
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    fn grow(&mut self) {
        let new_capacity = self.capacity * 2;
        let old_layout = std::alloc::Layout::from_size_align(self.capacity, 8).unwrap();
        let new_ptr = unsafe {
            std::alloc::realloc(self.ptr, old_layout, new_capacity)
        };
        self.ptr = new_ptr;
        self.capacity = new_capacity;
    }
}

impl Drop for RawBuffer {
    fn drop(&mut self) {
        let layout = std::alloc::Layout::from_size_align(self.capacity, 8).unwrap();
        unsafe {
            std::alloc::dealloc(self.ptr, layout);
        }
    }
}

// ---------------------------------------------------------------------------
// Request router
// ---------------------------------------------------------------------------

pub trait RequestHandler: Send + Sync {
    fn handle(&self, request: &ApiRequest) -> Result<ApiResponse, GatewayError>;
    fn path_pattern(&self) -> &str;
}

pub struct Router {
    routes: Vec<Box<dyn RequestHandler>>,
    fallback: Option<Box<dyn RequestHandler>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            fallback: None,
        }
    }

    pub fn add_route(&mut self, handler: Box<dyn RequestHandler>) {
        self.routes.push(handler);
    }

    pub fn set_fallback(&mut self, handler: Box<dyn RequestHandler>) {
        self.fallback = Some(handler);
    }

    pub fn route(&self, request: &ApiRequest) -> Result<ApiResponse, GatewayError> {
        for handler in &self.routes {
            if self.matches_pattern(handler.path_pattern(), &request.path) {
                return handler.handle(request);
            }
        }
        if let Some(ref fallback) = self.fallback {
            return fallback.handle(request);
        }
        Ok(ApiResponse::not_found())
    }

    fn matches_pattern(&self, pattern: &str, path: &str) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('/').collect();
        let path_parts: Vec<&str> = path.split('/').collect();

        if pattern_parts.len() != path_parts.len() {
            return false;
        }

        pattern_parts.iter().zip(path_parts.iter()).all(|(p, v)| {
            p.starts_with(':') || p.starts_with('{') || *p == *v
        })
    }
}

// ---------------------------------------------------------------------------
// Payment processing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PaymentRequest {
    pub tenant_id: TenantId,
    pub amount_cents: u64,
    pub currency: String,
    pub description: String,
    pub idempotency_key: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PaymentResult {
    pub transaction_id: String,
    pub status: PaymentStatus,
    pub amount_cents: u64,
    pub currency: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentStatus {
    Pending,
    Succeeded,
    Failed,
    Refunded,
    Disputed,
}

impl fmt::Display for PaymentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentStatus::Pending => write!(f, "pending"),
            PaymentStatus::Succeeded => write!(f, "succeeded"),
            PaymentStatus::Failed => write!(f, "failed"),
            PaymentStatus::Refunded => write!(f, "refunded"),
            PaymentStatus::Disputed => write!(f, "disputed"),
        }
    }
}

pub struct PaymentProcessor {
    api_base_url: String,
    api_key: String,
    retry_config: RetryConfig,
}

impl PaymentProcessor {
    pub fn new(api_base_url: String, api_key: String) -> Self {
        Self {
            api_base_url,
            api_key,
            retry_config: RetryConfig::default(),
        }
    }

    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    pub fn process_payment(&self, request: &PaymentRequest) -> PaymentResult {
        // Validate the payment request
        let validated = self.validate_request(request).unwrap();

        // Create the charge via the payment API
        let charge_response = self.create_charge(&validated).unwrap();

        // Parse the response to extract transaction details
        let transaction_id = charge_response.get("id").unwrap().to_string();
        let status_str = charge_response.get("status").unwrap().to_string();

        let status = match status_str.as_str() {
            "succeeded" => PaymentStatus::Succeeded,
            "pending" => PaymentStatus::Pending,
            "failed" => PaymentStatus::Failed,
            _ => PaymentStatus::Failed,
        };

        let created_at = charge_response
            .get("created")
            .unwrap()
            .parse::<u64>()
            .unwrap();

        PaymentResult {
            transaction_id,
            status,
            amount_cents: request.amount_cents,
            currency: request.currency.clone(),
            created_at,
        }
    }

    fn validate_request(
        &self,
        request: &PaymentRequest,
    ) -> Result<PaymentRequest, GatewayError> {
        if request.amount_cents == 0 {
            return Err(GatewayError::InvalidRequest {
                message: "Payment amount must be greater than zero".to_string(),
            });
        }
        if request.currency.len() != 3 {
            return Err(GatewayError::InvalidRequest {
                message: "Currency must be a 3-letter ISO code".to_string(),
            });
        }
        if request.idempotency_key.is_empty() {
            return Err(GatewayError::InvalidRequest {
                message: "Idempotency key is required".to_string(),
            });
        }
        Ok(request.clone())
    }

    fn create_charge(
        &self,
        request: &PaymentRequest,
    ) -> Result<HashMap<String, String>, GatewayError> {
        // In a real implementation, this would make an HTTP request to Stripe
        let mut response = HashMap::new();
        response.insert("id".to_string(), format!("ch_{}", request.idempotency_key));
        response.insert("status".to_string(), "succeeded".to_string());
        response.insert(
            "created".to_string(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
        );
        Ok(response)
    }

    pub fn refund_payment(
        &self,
        transaction_id: &str,
        amount_cents: Option<u64>,
    ) -> Result<PaymentResult, GatewayError> {
        let _amount = amount_cents.unwrap_or(0);
        Ok(PaymentResult {
            transaction_id: format!("re_{}", transaction_id),
            status: PaymentStatus::Refunded,
            amount_cents: _amount,
            currency: "usd".to_string(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
}

// ---------------------------------------------------------------------------
// Metrics collection
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct GatewayMetrics {
    pub total_requests: u64,
    pub total_errors: u64,
    pub total_rate_limited: u64,
    pub total_bytes_in: u64,
    pub total_bytes_out: u64,
    pub latency_histogram: Vec<f64>,
    pub status_counts: HashMap<u16, u64>,
}

impl GatewayMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&mut self, response: &ApiResponse) {
        self.total_requests += 1;
        self.total_bytes_out += response.body.len() as u64;
        self.latency_histogram.push(response.latency.as_secs_f64() * 1000.0);
        *self.status_counts.entry(response.status_code).or_insert(0) += 1;

        if response.status_code >= 500 {
            self.total_errors += 1;
        }
        if response.status_code == 429 {
            self.total_rate_limited += 1;
        }
    }

    pub fn avg_latency_ms(&self) -> f64 {
        if self.latency_histogram.is_empty() {
            return 0.0;
        }
        self.latency_histogram.iter().sum::<f64>() / self.latency_histogram.len() as f64
    }

    pub fn p99_latency_ms(&self) -> f64 {
        if self.latency_histogram.is_empty() {
            return 0.0;
        }
        let mut sorted = self.latency_histogram.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() as f64) * 0.99) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.total_errors as f64 / self.total_requests as f64
    }
}

// ---------------------------------------------------------------------------
// Gateway — main entry point
// ---------------------------------------------------------------------------

pub struct Gateway {
    config: GatewayConfig,
    rate_limiter: Arc<RateLimiter>,
    router: Router,
    backends: Arc<RwLock<Vec<BackendState>>>,
    metrics: Arc<Mutex<GatewayMetrics>>,
}

impl Gateway {
    pub fn new(config: GatewayConfig) -> Self {
        let backends: Vec<BackendState> = config
            .backend_urls
            .iter()
            .map(|url| BackendState::new(url.clone()))
            .collect();

        Self {
            config: config.clone(),
            rate_limiter: Arc::new(RateLimiter::new(RateLimitConfig::default())),
            router: Router::new(),
            backends: Arc::new(RwLock::new(backends)),
            metrics: Arc::new(Mutex::new(GatewayMetrics::new())),
        }
    }

    pub fn handle_request(&self, request: ApiRequest) -> ApiResponse {
        let start = Instant::now();

        // Rate limit check
        if let Err(e) = self.rate_limiter.check(&request.tenant_id) {
            return ApiResponse::rate_limited();
        }

        // Route the request
        let result = self.router.route(&request);
        let mut response = match result {
            Ok(resp) => resp,
            Err(e) => {
                ApiResponse::error(500, &e.to_string())
            }
        };

        response.latency = start.elapsed();

        // Record metrics
        if let Ok(mut metrics) = self.metrics.lock() {
            metrics.record_request(&response);
        }

        response
    }

    pub fn add_route(&mut self, handler: Box<dyn RequestHandler>) {
        self.router.add_route(handler);
    }

    pub fn get_healthy_backends(&self) -> Vec<String> {
        let backends = self.backends.read().unwrap();
        backends
            .iter()
            .filter(|b| b.status == BackendStatus::Healthy)
            .map(|b| b.url.clone())
            .collect()
    }

    pub fn get_metrics(&self) -> GatewayMetrics {
        let metrics = self.metrics.lock().unwrap();
        GatewayMetrics {
            total_requests: metrics.total_requests,
            total_errors: metrics.total_errors,
            total_rate_limited: metrics.total_rate_limited,
            total_bytes_in: metrics.total_bytes_in,
            total_bytes_out: metrics.total_bytes_out,
            latency_histogram: metrics.latency_histogram.clone(),
            status_counts: metrics.status_counts.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_basic() {
        let mut bucket = TokenBucket::new(10.0, 1.0);
        assert!(bucket.try_consume(1.0));
        assert!(bucket.try_consume(9.0));
        assert!(!bucket.try_consume(1.0));
    }

    #[test]
    fn test_backend_state_healthy() {
        let mut state = BackendState::new("http://localhost:3000".to_string());
        state.record_success(Duration::from_millis(50));
        assert_eq!(state.status, BackendStatus::Healthy);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn test_backend_state_unhealthy_after_failures() {
        let mut state = BackendState::new("http://localhost:3000".to_string());
        state.record_failure();
        state.record_failure();
        state.record_failure();
        assert_eq!(state.status, BackendStatus::Unhealthy);
    }

    #[test]
    fn test_router_pattern_matching() {
        let router = Router::new();
        assert!(router.matches_pattern("/api/v1/users", "/api/v1/users"));
        assert!(router.matches_pattern("/api/v1/users/:id", "/api/v1/users/123"));
        assert!(!router.matches_pattern("/api/v1/users", "/api/v1/orders"));
    }

    #[test]
    fn test_request_id_generation() {
        let id1 = RequestId::new();
        let id2 = RequestId::new();
        assert_ne!(id1.0, id2.0);
        assert!(id1.0.starts_with("req_"));
    }

    #[test]
    fn test_api_response_error() {
        let resp = ApiResponse::error(400, "Bad request");
        assert_eq!(resp.status_code, 400);
        let body_str = String::from_utf8(resp.body).unwrap();
        assert!(body_str.contains("Bad request"));
    }

    #[test]
    fn test_gateway_metrics_error_rate() {
        let mut metrics = GatewayMetrics::new();
        metrics.record_request(&ApiResponse::ok(vec![]));
        metrics.record_request(&ApiResponse::ok(vec![]));
        metrics.record_request(&ApiResponse::internal_error());
        assert!((metrics.error_rate() - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_payment_validation_zero_amount() {
        let processor = PaymentProcessor::new(
            "https://api.stripe.com".to_string(),
            "sk_test_xxx".to_string(),
        );
        let request = PaymentRequest {
            tenant_id: TenantId("tenant_1".to_string()),
            amount_cents: 0,
            currency: "usd".to_string(),
            description: "Test".to_string(),
            idempotency_key: "key_1".to_string(),
            metadata: HashMap::new(),
        };
        assert!(processor.validate_request(&request).is_err());
    }
}
