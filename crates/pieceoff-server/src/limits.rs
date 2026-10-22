//! Per-client limits for a server on the open internet: a token-bucket rate
//! limiter, where a client's address comes from, and the browser-origin check.

use axum::http::HeaderMap;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

/// A token bucket per client address. Each address starts with `capacity`
/// tokens and earns them back at `per_second`; an action spends some.
pub struct RateLimiter {
    capacity: f64,
    per_second: f64,
    buckets: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

impl RateLimiter {
    /// Allow `count` actions per `period`, in bursts of up to `count`.
    pub fn per_period(count: u32, period: Duration) -> Self {
        let capacity = f64::from(count.max(1));
        Self {
            capacity,
            per_second: capacity / period.as_secs_f64().max(1.0),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Spend `cost` tokens if the address has them.
    pub fn try_take(&self, address: IpAddr, cost: f64) -> bool {
        self.try_take_at(address, cost, Instant::now())
    }

    pub fn try_take_at(&self, address: IpAddr, cost: f64, now: Instant) -> bool {
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (tokens, last) = buckets.entry(address).or_insert((self.capacity, now));
        spend(tokens, last, self.capacity, self.per_second, cost, now)
    }

    /// Whether the address could spend `cost` right now, without spending.
    pub fn would_allow(&self, address: IpAddr, cost: f64) -> bool {
        let buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        buckets.get(&address).is_none_or(|(tokens, last)| {
            let earned = last.elapsed().as_secs_f64() * self.per_second;
            (*tokens + earned).min(self.capacity) >= cost
        })
    }

    /// Forget addresses whose buckets have refilled: they are
    /// indistinguishable from addresses never seen.
    pub fn forget_idle(&self) {
        let now = Instant::now();
        self.buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, (tokens, last)| {
                let earned = now.saturating_duration_since(*last).as_secs_f64() * self.per_second;
                *tokens + earned < self.capacity
            });
    }

    #[cfg(test)]
    pub fn tracked_addresses(&self) -> usize {
        self.buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// Refill a bucket for the time since `last`, then spend `cost` if it is there.
fn spend(
    tokens: &mut f64,
    last: &mut Instant,
    capacity: f64,
    per_second: f64,
    cost: f64,
    now: Instant,
) -> bool {
    let earned = now.saturating_duration_since(*last).as_secs_f64() * per_second;
    *tokens = (*tokens + earned).min(capacity);
    *last = now;
    if *tokens >= cost {
        *tokens -= cost;
        true
    } else {
        false
    }
}

/// A single token bucket owned by one connection, for messages that are
/// limited per player rather than per address (live cursors).
pub struct TokenBucket {
    capacity: f64,
    per_second: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// Allow `per_second` actions a second, in bursts of up to `burst`.
    pub fn new(per_second: f64, burst: f64, now: Instant) -> Self {
        Self {
            capacity: burst.max(1.0),
            per_second,
            tokens: burst.max(1.0),
            last: now,
        }
    }

    pub fn try_take_at(&mut self, now: Instant) -> bool {
        spend(
            &mut self.tokens,
            &mut self.last,
            self.capacity,
            self.per_second,
            1.0,
            now,
        )
    }
}

/// The address a request really came from. Behind a trusted reverse proxy
/// that is the last `X-Forwarded-For` entry (the one the proxy itself
/// appended; earlier entries are whatever the client claimed). Otherwise it is
/// the socket peer. `None` when neither is known.
pub fn client_address(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trust_proxy: bool,
) -> Option<IpAddr> {
    if trust_proxy {
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .rfind(|entry| !entry.is_empty())
            .and_then(|entry| entry.parse::<IpAddr>().ok());
        if forwarded.is_some() {
            return forwarded;
        }
    }
    peer.map(|peer| peer.ip())
}

/// Browsers attach an `Origin` to a WebSocket handshake, and a page on any
/// site can open a socket to a server on the visitor's own network. Native
/// clients send none and are allowed. A browser origin must be listed, or be
/// the server's own host.
pub fn origin_allowed(origin: Option<&str>, host: Option<&str>, allowed: &[String]) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
    if allowed.contains(&origin) {
        return true;
    }
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    match (authority, host) {
        (Some(authority), Some(host)) => authority == host.trim().to_ascii_lowercase(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn a_bucket_allows_a_burst_then_refills_over_time() {
        let limiter = RateLimiter::per_period(3, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.try_take_at(ip(1), 1.0, start));
        assert!(limiter.try_take_at(ip(1), 1.0, start));
        assert!(limiter.try_take_at(ip(1), 1.0, start));
        assert!(
            !limiter.try_take_at(ip(1), 1.0, start),
            "burst of three is spent"
        );
        // Three per minute is one token every twenty seconds.
        assert!(!limiter.try_take_at(ip(1), 1.0, start + Duration::from_secs(19)));
        assert!(limiter.try_take_at(ip(1), 1.0, start + Duration::from_secs(21)));
        // A refill never banks more than the burst.
        let later = start + Duration::from_secs(3600);
        for _ in 0..3 {
            assert!(limiter.try_take_at(ip(1), 1.0, later));
        }
        assert!(!limiter.try_take_at(ip(1), 1.0, later));
    }

    #[test]
    fn a_connection_bucket_caps_a_steady_rate_after_its_burst() {
        let start = Instant::now();
        let mut bucket = TokenBucket::new(20.0, 5.0, start);
        for _ in 0..5 {
            assert!(bucket.try_take_at(start));
        }
        assert!(!bucket.try_take_at(start), "burst of five is spent");
        // Twenty a second is one token every 50 ms.
        assert!(!bucket.try_take_at(start + Duration::from_millis(40)));
        assert!(bucket.try_take_at(start + Duration::from_millis(60)));
        let later = start + Duration::from_secs(60);
        let allowed = (0..100).filter(|_| bucket.try_take_at(later)).count();
        assert_eq!(allowed, 5, "a long pause banks only the burst");
    }

    #[test]
    fn addresses_have_separate_buckets_and_costs_can_exceed_one_token() {
        let limiter = RateLimiter::per_period(2, Duration::from_secs(60));
        let now = Instant::now();
        assert!(limiter.try_take_at(ip(1), 2.0, now));
        assert!(!limiter.try_take_at(ip(1), 1.0, now));
        assert!(
            limiter.try_take_at(ip(2), 1.0, now),
            "another address is unaffected"
        );
        assert!(
            !limiter.try_take_at(ip(3), 3.0, now),
            "more than the burst never fits"
        );
    }

    #[test]
    fn idle_addresses_are_forgotten_but_spent_ones_are_kept() {
        let limiter = RateLimiter::per_period(2, Duration::from_secs(1));
        let long_ago = Instant::now() - Duration::from_secs(120);
        assert!(limiter.try_take_at(ip(1), 1.0, long_ago));
        assert!(limiter.try_take(ip(2), 2.0));
        assert_eq!(limiter.tracked_addresses(), 2);
        limiter.forget_idle();
        assert_eq!(
            limiter.tracked_addresses(),
            1,
            "only the drained bucket stays"
        );
        assert!(!limiter.would_allow(ip(2), 1.0));
        assert!(limiter.would_allow(ip(9), 1.0));
    }

    #[test]
    fn the_client_address_trusts_the_proxy_only_when_told_to() {
        let peer = Some(SocketAddr::from(([10, 0, 0, 5], 4000)));
        let mut headers = HeaderMap::new();
        headers.append(
            "x-forwarded-for",
            "198.51.100.9, 203.0.113.7".parse().unwrap(),
        );
        // Without a trusted proxy the header is just a claim.
        assert_eq!(
            client_address(peer, &headers, false),
            Some(IpAddr::from([10, 0, 0, 5]))
        );
        // With one, the entry the proxy appended (the last) is the real client;
        // the first is whatever the client wrote.
        assert_eq!(
            client_address(peer, &headers, true),
            Some(IpAddr::from([203, 0, 113, 7]))
        );
        // Junk falls back to the socket.
        let mut junk = HeaderMap::new();
        junk.append("x-forwarded-for", "not-an-address".parse().unwrap());
        assert_eq!(
            client_address(peer, &junk, true),
            Some(IpAddr::from([10, 0, 0, 5]))
        );
        assert_eq!(client_address(None, &HeaderMap::new(), true), None);
    }

    #[test]
    fn browser_origins_must_be_listed_or_match_the_host() {
        let allowed = vec!["https://play.example.com".to_owned()];
        // Native clients send no Origin.
        assert!(origin_allowed(None, Some("rooms.example.com"), &allowed));
        assert!(origin_allowed(
            Some("https://play.example.com"),
            None,
            &allowed
        ));
        assert!(origin_allowed(
            Some("HTTPS://Play.Example.com/"),
            None,
            &allowed
        ));
        // The server's own pages are same-origin.
        assert!(origin_allowed(
            Some("https://rooms.example.com"),
            Some("rooms.example.com"),
            &[]
        ));
        // Any other site is refused, including look-alikes.
        assert!(!origin_allowed(
            Some("https://evil.example.org"),
            Some("rooms.example.com"),
            &allowed
        ));
        assert!(!origin_allowed(
            Some("https://play.example.com.evil.org"),
            None,
            &allowed
        ));
        assert!(!origin_allowed(
            Some("null"),
            Some("rooms.example.com"),
            &allowed
        ));
    }
}
