use serde::{Deserialize, Serialize};

/// A glob-based action pattern (e.g. `"read:*"`, `"write:/home/**"`).
///
/// Uses `glob::Pattern` for matching action strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPattern {
    pattern: String,
}

impl ActionPattern {
    /// Create a new action pattern. Returns `None` if the pattern is invalid.
    pub fn new(pattern: &str) -> Option<Self> {
        // Validate that the glob pattern compiles.
        glob::Pattern::new(pattern).ok()?;
        Some(Self {
            pattern: pattern.to_owned(),
        })
    }

    /// Test whether `action` matches this pattern.
    pub fn matches(&self, action: &str) -> bool {
        glob::Pattern::new(&self.pattern)
            .map(|p| p.matches(action))
            .unwrap_or(false)
    }

    /// Returns `true` if every string matched by `self` would also be matched
    /// by `other`.
    ///
    /// Exact subset analysis of arbitrary globs is intractable, so we use a
    /// conservative approximation: `self` is a subset of `other` when the
    /// patterns are equal, or when `other` is the universal wildcard `"*"`.
    pub fn is_subset_of(&self, other: &ActionPattern) -> bool {
        self.pattern == other.pattern || other.pattern == "*"
    }

    /// The raw pattern string.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }
}

impl PartialEq for ActionPattern {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}
impl Eq for ActionPattern {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_any() {
        let pat = ActionPattern::new("*").unwrap();
        assert!(pat.matches("read"));
        assert!(pat.matches("write"));
        assert!(pat.matches("anything"));
    }

    #[test]
    fn prefix_glob_matches() {
        let pat = ActionPattern::new("read:*").unwrap();
        assert!(pat.matches("read:foo"));
        assert!(pat.matches("read:bar"));
        assert!(!pat.matches("write:foo"));
    }

    #[test]
    fn exact_match() {
        let pat = ActionPattern::new("deploy").unwrap();
        assert!(pat.matches("deploy"));
        assert!(!pat.matches("deploy:prod"));
    }

    #[test]
    fn invalid_pattern_returns_none() {
        assert!(ActionPattern::new("[invalid").is_none());
    }

    #[test]
    fn subset_equal_patterns() {
        let a = ActionPattern::new("read:*").unwrap();
        let b = ActionPattern::new("read:*").unwrap();
        assert!(a.is_subset_of(&b));
    }

    #[test]
    fn subset_of_universal() {
        let a = ActionPattern::new("read:*").unwrap();
        let b = ActionPattern::new("*").unwrap();
        assert!(a.is_subset_of(&b));
    }

    #[test]
    fn not_subset_of_different() {
        let a = ActionPattern::new("read:*").unwrap();
        let b = ActionPattern::new("write:*").unwrap();
        assert!(!a.is_subset_of(&b));
    }

    #[test]
    fn universal_not_subset_of_narrow() {
        let a = ActionPattern::new("*").unwrap();
        let b = ActionPattern::new("read:*").unwrap();
        assert!(!a.is_subset_of(&b));
    }
}
