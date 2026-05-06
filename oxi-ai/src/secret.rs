//! 민감 정보(API 키 등)의 실수 노출을 방지하는 래퍼 타입.
//!
//! `Secret<T>`는 [`Debug`], [`Display`] 구현에서 값을 마스킹한다.
//! 실제 값은 [`expose()`](Secret::expose)로만 접근 가능하다.

use std::fmt;

/// API 키 등 민감 정보를 감싸는 타입.
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
    /// 민감 값을 감싼다.
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// 실제 값에 접근. HTTP 헤더 생성 등 필수 로직에서만 사용.
    pub fn expose(&self) -> &T {
        &self.inner
    }

    /// 소유권 이전으로 값 꺼내기.
    pub fn expose_owned(self) -> T {
        self.inner
    }

    /// 내부 값 변환.
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
        s.serialize_str(&self.inner)
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
        assert!(json.contains("sk-test-123"));
        let back: Secret<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.expose(), "sk-test-123");
    }
}
