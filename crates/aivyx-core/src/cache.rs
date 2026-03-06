//! Trait for cache-friendly types.
//!
//! Types implementing [`Cacheable`] declare their own cache key and TTL,
//! enabling generic caching infrastructure to work with heterogeneous types.

use std::time::Duration;

/// Trait for types that can be cached.
///
/// Implementors declare a cache key (used for lookup/dedup) and a TTL
/// (how long the cached value remains valid).
pub trait Cacheable {
    /// Return a string key suitable for cache lookup.
    fn cache_key(&self) -> String;

    /// Return the time-to-live for cached values of this type.
    fn cache_ttl(&self) -> Duration;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SearchQuery {
        query: String,
    }

    impl Cacheable for SearchQuery {
        fn cache_key(&self) -> String {
            format!("search:{}", self.query)
        }

        fn cache_ttl(&self) -> Duration {
            Duration::from_secs(300)
        }
    }

    #[test]
    fn cacheable_trait_can_be_implemented() {
        let q = SearchQuery {
            query: "rust async".into(),
        };
        assert_eq!(q.cache_key(), "search:rust async");
        assert_eq!(q.cache_ttl(), Duration::from_secs(300));
    }
}
