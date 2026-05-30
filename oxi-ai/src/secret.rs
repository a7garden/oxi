//! Wrapper type to prevent accidental exposure of sensitive data (e.g., API keys).
//!
//! `Secret<T>` masks values in `Debug` and `Display` implementations.
//! The actual value is only accessible via [`expose()`](Secret::expose).

use std::fmt;

/// A wrapper for sensitive data such as API keys.
///
/// # Examples
/// ```ignore
/// let key = Secret::new("sk-ant-1234567890abcdef".to_string());
/// println!("{key:?}");   // Secret([REDACTED])
/// println!("{key}");     // sk-an...cdef
/// let header = format!("Bearer {}", key.expose());
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Secret<T> {
    inner: T,
}

impl<T> Secret<T> {
    /// Wraps a sensitive value.
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// Access the underlying value. Use only in essential logic such as HTTP header generation.
    pub fn expose(&self) -> &T {
        &self.inner
    }

    /// Extract the value by transferring ownership.
    pub fn expose_owned(self) -> T {
        self.inner
    }

    /// Transform the inner value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Secret<U> {
        Secret::new(f(self.inner))
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl fmt::Display for Secret<String> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.inner;
        if s.len() > 8 {
            write!(f, "{}...{}", &s[..4], &s[s.len() - 4..])
        } else {
            f.write_str("[REDACTED]")
        }
    }
}

impl serde::Serialize for Secret<String> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Don't serialize the actual secret value
        s.serialize_str("[REDACTED]")
    }
}

impl<'de> serde::Deserialize<'de> for Secret<String> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::new(String::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_masks_value() {
        let s = Secret::new("sk-ant-12345678".to_string());
        assert_eq!(format!("{s:?}"), "Secret([REDACTED])");
    }

    #[test]
    fn display_short_value_redacted() {
        let s = Secret::new("short".to_string());
        assert_eq!(format!("{s}"), "[REDACTED]");
    }

    #[test]
    fn display_long_value_partial() {
        let s = Secret::new("sk-ant-12345678".to_string());
        assert_eq!(format!("{s}"), "sk-a...5678");
    }

    #[test]
    fn expose_returns_inner() {
        let s = Secret::new("secret-key".to_string());
        assert_eq!(s.expose(), "secret-key");
    }

    #[test]
    fn expose_owned_consumes() {
        let s = Secret::new("my-key".to_string());
        let inner = s.expose_owned();
        assert_eq!(inner, "my-key");
    }

    #[test]
    fn map_transforms() {
        let s = Secret::new("key".to_string());
        let mapped = s.map(|v| v.len());
        assert_eq!(*mapped.expose(), 3);
    }

    #[test]
    fn serde_roundtrip() {
        let s = Secret::new("sk-test-123".to_string());
        let json = serde_json::to_string(&s).unwrap();
        // Serialize should mask the value
        assert!(json.contains("[REDACTED]"));
        assert!(!json.contains("sk-test-123"));
        // Deserialize still works (from non-redacted JSON)
        let input_json = r#""sk-test-123""#;
        let back: Secret<String> = serde_json::from_str(input_json).unwrap();
        assert_eq!(back.expose(), "sk-test-123");
    }
}
