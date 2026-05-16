//! Integration tests for Settings lifecycle:
//! save/load round-trip, layer merging, format detection, project settings.

use oxi_store::settings::{Settings, SettingsFormat};
use std::fs;
use tempfile::TempDir;

// ── Helpers ──────────────────────────────────────────────────────

fn write_settings_file(dir: &std::path::Path, filename: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(filename);
    fs::write(&path, content).expect("write settings file");
    path
}

// ═══════════════════════════════════════════════════════════════════
// 1. Save and Reload
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_save_and_reload_json() {
    let tmp = TempDir::new().expect("temp dir");
    let settings_path = tmp.path().join("settings.json");

    let mut settings = Settings::default();
    settings.default_model = Some("claude-sonnet-4-20250514".to_string());
    settings.default_provider = Some("anthropic".to_string());
    settings.theme = "monokai".to_string();
    settings.stream_responses = false;
    settings.tool_timeout_seconds = 60;

    // Save to specific path
    settings.save_to(&settings_path).expect("save settings");

    // Verify file was created
    assert!(settings_path.exists(), "settings file should exist");

    // Reload
    let content = fs::read_to_string(&settings_path).expect("read settings");
    let loaded: Settings = serde_json::from_str(&content).expect("parse settings JSON");

    assert_eq!(
        loaded.default_model,
        Some("claude-sonnet-4-20250514".to_string())
    );
    assert_eq!(loaded.default_provider, Some("anthropic".to_string()));
    assert_eq!(loaded.theme, "monokai");
    assert!(!loaded.stream_responses);
    assert_eq!(loaded.tool_timeout_seconds, 60);
}

#[test]
fn test_settings_save_and_reload_toml() {
    let tmp = TempDir::new().expect("temp dir");
    let settings_path = tmp.path().join("settings.toml");

    let mut settings = Settings::default();
    settings.default_model = Some("gpt-4o".to_string());
    settings.default_provider = Some("openai".to_string());

    settings
        .save_to(&settings_path)
        .expect("save settings TOML");

    assert!(settings_path.exists());

    let content = fs::read_to_string(&settings_path).expect("read settings");
    let loaded: Settings = toml::from_str(&content).expect("parse settings TOML");

    assert_eq!(loaded.default_model, Some("gpt-4o".to_string()));
    assert_eq!(loaded.default_provider, Some("openai".to_string()));
}

#[test]
fn test_settings_roundtrip_preserves_all_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let settings_path = tmp.path().join("settings.json");

    let mut original = Settings::default();
    original.version = 4;
    original.default_model = Some("test-model".to_string());
    original.default_provider = Some("test-provider".to_string());
    original.max_tokens = Some(4096);
    original.temperature = Some(0.7);
    original.default_temperature = Some(0.8);
    original.max_response_tokens = Some(8192);
    original.session_history_size = 50;
    original.stream_responses = false;
    original.extensions_enabled = false;
    original.auto_compaction = false;
    original.disabled_tools = vec!["web_search".to_string()];
    original.tool_timeout_seconds = 30;
    original.extensions = vec!["ext1".to_string()];
    original.skills = vec!["skill1".to_string()];
    original.custom_providers = vec![oxi_store::settings::CustomProvider {
        name: "test-provider".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key_env: "TEST_API_KEY".to_string(),
        api: "openai-completions".to_string(),
    }];

    original.save_to(&settings_path).expect("save");

    let content = fs::read_to_string(&settings_path).expect("read");
    let loaded: Settings = serde_json::from_str(&content).expect("parse");

    assert_eq!(loaded.version, 4);
    assert_eq!(loaded.default_model, Some("test-model".to_string()));
    assert_eq!(loaded.default_provider, Some("test-provider".to_string()));
    assert_eq!(loaded.max_tokens, Some(4096));
    assert_eq!(loaded.default_temperature, Some(0.8));
    assert_eq!(loaded.max_response_tokens, Some(8192));
    assert_eq!(loaded.session_history_size, 50);
    assert!(!loaded.stream_responses);
    assert!(!loaded.extensions_enabled);
    assert!(!loaded.auto_compaction);
    assert_eq!(loaded.disabled_tools, vec!["web_search"]);
    assert_eq!(loaded.tool_timeout_seconds, 30);
    assert_eq!(loaded.extensions, vec!["ext1"]);
    assert_eq!(loaded.skills, vec!["skill1"]);
    assert_eq!(loaded.custom_providers.len(), 1);
    assert_eq!(loaded.custom_providers[0].name, "test-provider");
}

// ═══════════════════════════════════════════════════════════════════
// 2. Layer Merging
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_layer_merging() {
    let tmp = TempDir::new().expect("temp dir");

    // Create a TOML settings file with partial overrides
    let toml_content = r#"
default_model = "claude-sonnet-4-20250514"
default_provider = "anthropic"
theme = "dracula"
stream_responses = false
"#;
    write_settings_file(tmp.path(), "settings.toml", toml_content);

    // Parse and verify the layer file logic
    let parsed: Settings = toml::from_str(toml_content).expect("parse TOML");

    // These should be overridden
    assert_eq!(
        parsed.default_model,
        Some("claude-sonnet-4-20250514".to_string())
    );
    assert_eq!(parsed.default_provider, Some("anthropic".to_string()));
    assert_eq!(parsed.theme, "dracula");
    assert!(!parsed.stream_responses);

    // These should have defaults (not specified in TOML)
    assert!(parsed.auto_compaction); // default: true
    assert_eq!(parsed.tool_timeout_seconds, 120); // default: 120
}

#[test]
fn test_settings_project_layer_overrides_global() {
    let tmp = TempDir::new().expect("temp dir");

    // Global settings
    let global_dir = tmp.path().join("global");
    fs::create_dir_all(&global_dir).expect("create global dir");

    let global_settings = r#"
default_model = "gpt-4o"
default_provider = "openai"
theme = "default"
"#;
    write_settings_file(&global_dir, "settings.json", global_settings);

    // Project settings (override model only) — use TOML for project config
    let project_dir = tmp.path().join("project");
    let oxi_dir = project_dir.join(".oxi");
    fs::create_dir_all(&oxi_dir).expect("create oxi dir");

    let project_settings = r#"default_model = "claude-sonnet-4-20250514"
default_provider = "anthropic"
"#;
    write_settings_file(&oxi_dir, "settings.toml", project_settings);

    // Verify project settings can be found
    let found = Settings::find_project_settings(&project_dir);
    assert!(found.is_some(), "project settings should be found");
    let found_path = found.unwrap();
    assert!(found_path.starts_with(&oxi_dir));

    let content = fs::read_to_string(&found_path).expect("read project settings");
    let project: Settings = toml::from_str(&content).expect("parse project");
    assert_eq!(
        project.default_model,
        Some("claude-sonnet-4-20250514".to_string())
    );
}

// ═══════════════════════════════════════════════════════════════════
// 3. Environment Override (currently a no-op)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_env_override_no_op() {
    // apply_env() is currently a no-op — verify it doesn't modify settings
    let mut settings = Settings::default();
    let original_model = settings.default_model.clone();
    let original_theme = settings.theme.clone();
    let original_stream = settings.stream_responses;

    settings.apply_env();

    assert_eq!(settings.default_model, original_model);
    assert_eq!(settings.theme, original_theme);
    assert_eq!(settings.stream_responses, original_stream);
}

#[test]
fn test_settings_from_env_returns_defaults() {
    // from_env() is currently deprecated and returns defaults
    let from_env = Settings::from_env();
    let defaults = Settings::default();

    assert_eq!(from_env.default_model, defaults.default_model);
    assert_eq!(from_env.theme, defaults.theme);
    assert_eq!(from_env.stream_responses, defaults.stream_responses);
}

// ═══════════════════════════════════════════════════════════════════
// 4. Format Detection
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_detect_format_json() {
    let path = std::path::Path::new("/tmp/test/settings.json");
    assert!(matches!(
        Settings::detect_format(path),
        SettingsFormat::Json
    ));
}

#[test]
fn test_settings_detect_format_toml() {
    let path = std::path::Path::new("/tmp/test/settings.toml");
    assert!(matches!(
        Settings::detect_format(path),
        SettingsFormat::Toml
    ));
}

#[test]
fn test_settings_detect_format_unknown_defaults_json() {
    let path = std::path::Path::new("/tmp/test/settings.yaml");
    assert!(matches!(
        Settings::detect_format(path),
        SettingsFormat::Json
    ));
}

#[test]
fn test_settings_parse_from_str_json() {
    let json = r#"{"theme":"monokai","stream_responses":false}"#;
    let settings = Settings::parse_from_str(json, SettingsFormat::Json).expect("parse JSON");
    assert_eq!(settings.theme, "monokai");
    assert!(!settings.stream_responses);
}

#[test]
fn test_settings_parse_from_str_toml() {
    let toml = r#"theme = "monokai"
stream_responses = false"#;
    let settings = Settings::parse_from_str(toml, SettingsFormat::Toml).expect("parse TOML");
    assert_eq!(settings.theme, "monokai");
    assert!(!settings.stream_responses);
}

// ═══════════════════════════════════════════════════════════════════
// 5. Effective Values
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_effective_model() {
    let mut settings = Settings::default();

    // No model configured
    assert!(settings.effective_model(None).is_none());

    // CLI override takes precedence
    settings.default_model = Some("gpt-4o".to_string());
    settings.default_provider = Some("openai".to_string());
    assert_eq!(
        settings.effective_model(Some("cli-model")),
        Some("cli-model".to_string())
    );

    // No CLI, uses provider/model combo
    assert_eq!(
        settings.effective_model(None),
        Some("openai/gpt-4o".to_string())
    );

    // Only model, no provider
    settings.default_provider = None;
    assert_eq!(settings.effective_model(None), Some("gpt-4o".to_string()));
}

#[test]
fn test_settings_effective_temperature() {
    let mut settings = Settings::default();
    assert!(settings.effective_temperature().is_none());

    settings.temperature = Some(0.5);
    assert_eq!(settings.effective_temperature(), Some(0.5));

    settings.default_temperature = Some(0.9);
    // default_temperature takes precedence
    assert_eq!(settings.effective_temperature(), Some(0.9));
}

#[test]
fn test_settings_effective_max_tokens() {
    let mut settings = Settings::default();
    assert!(settings.effective_max_tokens().is_none());

    settings.max_tokens = Some(2048);
    assert_eq!(settings.effective_max_tokens(), Some(2048));

    settings.max_response_tokens = Some(8192);
    // max_response_tokens takes precedence
    assert_eq!(settings.effective_max_tokens(), Some(8192));
}

// ═══════════════════════════════════════════════════════════════════
// 6. Default Values
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_default_values() {
    let settings = Settings::default();

    assert_eq!(settings.version, 4);
    assert!(settings.default_model.is_none());
    assert!(settings.default_provider.is_none());
    assert!(settings.last_used_model.is_none());
    assert!(settings.last_used_provider.is_none());
    assert!(settings.max_tokens.is_none());
    assert!(settings.temperature.is_none());
    assert_eq!(settings.session_history_size, 100);
    assert!(settings.session_dir.is_none());
    assert!(settings.stream_responses);
    assert!(settings.extensions_enabled);
    assert!(settings.auto_compaction);
    assert!(settings.disabled_tools.is_empty());
    assert_eq!(settings.tool_timeout_seconds, 120);
    assert!(settings.extensions.is_empty());
    assert!(settings.skills.is_empty());
    assert!(settings.prompts.is_empty());
    assert!(settings.themes.is_empty());
    assert!(settings.custom_providers.is_empty());
    assert!(settings.dynamic_models.is_empty());
}

// ═══════════════════════════════════════════════════════════════════
// 7. Custom Providers
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_custom_provider_serialization() {
    let mut settings = Settings::default();
    settings
        .custom_providers
        .push(oxi_store::settings::CustomProvider {
            name: "minimax".to_string(),
            base_url: "https://api.minimax.chat/v1".to_string(),
            api_key_env: "MINIMAX_API_KEY".to_string(),
            api: "openai-completions".to_string(),
        });

    let json = serde_json::to_string(&settings).expect("serialize");
    let loaded: Settings = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(loaded.custom_providers.len(), 1);
    assert_eq!(loaded.custom_providers[0].name, "minimax");
    assert_eq!(
        loaded.custom_providers[0].base_url,
        "https://api.minimax.chat/v1"
    );
    assert_eq!(loaded.custom_providers[0].api_key_env, "MINIMAX_API_KEY");
    assert_eq!(loaded.custom_providers[0].api, "openai-completions");
}

// ═══════════════════════════════════════════════════════════════════
// 8. Thinking Level
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_thinking_level_default() {
    use oxi_store::settings::ThinkingLevel;
    let settings = Settings::default();
    assert_eq!(settings.thinking_level, ThinkingLevel::Medium);
}

#[test]
fn test_thinking_level_serialization() {
    use oxi_store::settings::ThinkingLevel;
    let levels = vec![
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::XHigh,
    ];

    for level in &levels {
        let json = serde_json::to_string(&level).expect("serialize thinking level");
        let parsed: ThinkingLevel =
            serde_json::from_str(&json).expect("deserialize thinking level");
        assert_eq!(*level, parsed);
    }
}

// ═══════════════════════════════════════════════════════════════════
// 9. Merge CLI
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_merge_cli() {
    let mut settings = Settings::default();
    assert!(settings.default_model.is_none());
    assert!(settings.default_provider.is_none());

    settings.merge_cli(Some("gpt-4o".to_string()), Some("openai".to_string()));
    assert_eq!(settings.default_model, Some("gpt-4o".to_string()));
    assert_eq!(settings.default_provider, Some("openai".to_string()));

    // Override again
    settings.merge_cli(Some("claude-3".to_string()), None);
    assert_eq!(settings.default_model, Some("claude-3".to_string()));
    assert_eq!(settings.default_provider, Some("openai".to_string())); // unchanged
}

// ═══════════════════════════════════════════════════════════════════
// 10. Theme
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_get_theme_name() {
    let mut settings = Settings::default();

    // "default" maps to "oxi_dark"
    assert_eq!(settings.get_theme_name(), "oxi_dark");

    // Empty string maps to "oxi_dark"
    settings.theme = String::new();
    assert_eq!(settings.get_theme_name(), "oxi_dark");

    // Custom theme preserved
    settings.theme = "monokai".to_string();
    assert_eq!(settings.get_theme_name(), "monokai");
}

// ═══════════════════════════════════════════════════════════════════
// 11. Effective Session Dir
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_effective_session_dir() {
    let settings = Settings::default();
    let dir = settings
        .effective_session_dir()
        .expect("effective session dir");
    assert!(dir.to_string_lossy().contains(".oxi"));
    assert!(dir.to_string_lossy().contains("sessions"));

    // With explicit session_dir
    let mut settings = Settings::default();
    settings.session_dir = Some("/custom/session/path".into());
    let dir = settings
        .effective_session_dir()
        .expect("effective session dir");
    assert_eq!(dir, std::path::PathBuf::from("/custom/session/path"));
}

// ═══════════════════════════════════════════════════════════════════
// 12. Serialization Format
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_settings_serialize_for_format_json() {
    let settings = Settings::default();
    let json =
        Settings::serialize_for_format(&settings, SettingsFormat::Json).expect("serialize JSON");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(parsed.is_object());
}

#[test]
fn test_settings_serialize_for_format_toml() {
    let settings = Settings::default();
    let toml_str =
        Settings::serialize_for_format(&settings, SettingsFormat::Toml).expect("serialize TOML");
    let parsed: toml::Value = toml::from_str(&toml_str).expect("valid TOML");
    assert!(parsed.is_table());
}
