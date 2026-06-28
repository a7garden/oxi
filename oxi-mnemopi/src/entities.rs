//! Entity extraction — ported from omp `core/entities.ts`.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntityType {
    Email,
    Url,
    IpAddress,
    FilePath,
    Version,
    Uuid,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Url => "url",
            Self::IpAddress => "ip",
            Self::FilePath => "file_path",
            Self::Version => "version",
            Self::Uuid => "uuid",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub entity_type: EntityType,
    pub value: String,
}

struct EntityPatterns {
    email: Regex,
    url: Regex,
    ipv4: Regex,
    file_path: Regex,
    version: Regex,
    uuid: Regex,
}

static PATTERNS: LazyLock<EntityPatterns> = LazyLock::new(|| EntityPatterns {
    email: Regex::new(r#"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"#).unwrap(),
    url: Regex::new(r#"\bhttps?://[^\s<>"]+"#).unwrap(),
    ipv4: Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(),
    file_path: Regex::new(r"\b(?:[\w./-]+/)+[\w.-]+\b").unwrap(),
    version: Regex::new(r"\bv?\d+\.\d+\.\d+(?:-[a-zA-Z0-9.]+)?\b").unwrap(),
    uuid: Regex::new(
        r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
    )
    .unwrap(),
});

/// Extract all entities from text.
pub fn extract_entities(text: &str) -> Vec<Entity> {
    let mut results = Vec::new();

    for m in PATTERNS.email.find_iter(text) {
        results.push(Entity {
            entity_type: EntityType::Email,
            value: m.as_str().to_string(),
        });
    }
    for m in PATTERNS.url.find_iter(text) {
        results.push(Entity {
            entity_type: EntityType::Url,
            value: m.as_str().to_string(),
        });
    }
    for m in PATTERNS.ipv4.find_iter(text) {
        if m.as_str().split('.').all(|o| o.parse::<u8>().is_ok()) {
            results.push(Entity {
                entity_type: EntityType::IpAddress,
                value: m.as_str().to_string(),
            });
        }
    }
    for m in PATTERNS.version.find_iter(text) {
        results.push(Entity {
            entity_type: EntityType::Version,
            value: m.as_str().to_string(),
        });
    }
    for m in PATTERNS.uuid.find_iter(text) {
        results.push(Entity {
            entity_type: EntityType::Uuid,
            value: m.as_str().to_string(),
        });
    }
    for m in PATTERNS.file_path.find_iter(text) {
        if m.as_str().contains('/') || m.as_str().contains('.') {
            results.push(Entity {
                entity_type: EntityType::FilePath,
                value: m.as_str().to_string(),
            });
        }
    }

    results
}

/// Extract entities as a map of type name to values.
pub fn extract_entity_map(text: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for entity in extract_entities(text) {
        map.entry(entity.entity_type.as_str().to_string())
            .or_default()
            .push(entity.value);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_email() {
        let e = extract_entities("Contact user@example.com");
        assert!(
            e.iter()
                .any(|e| e.entity_type == EntityType::Email && e.value == "user@example.com")
        );
    }

    #[test]
    fn extract_url() {
        let e = extract_entities("See https://example.com/docs");
        assert!(e.iter().any(|e| e.entity_type == EntityType::Url));
    }

    #[test]
    fn extract_version() {
        let e = extract_entities("Upgraded to v1.2.3");
        assert!(
            e.iter()
                .any(|e| e.entity_type == EntityType::Version && e.value == "v1.2.3")
        );
    }

    #[test]
    fn extract_uuid() {
        let e = extract_entities("id: 550e8400-e29b-41d4-a716-446655440000");
        assert!(e.iter().any(|e| e.entity_type == EntityType::Uuid));
    }
}
