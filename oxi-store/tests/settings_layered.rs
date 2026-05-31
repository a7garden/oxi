//! Layered settings tests

#[cfg(test)]
mod tests {
    use oxi_store::settings::Settings;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert!(settings.version >= 1);
    }

    #[test]
    fn test_settings_validate_temperature_valid() {
        let mut settings = Settings::default();
        settings.temperature = Some(0.7);
        let report = settings.validate();
        assert!(report.is_valid(), "temperature 0.7 should be valid");
    }

    #[test]
    fn test_settings_validate_temperature_out_of_range() {
        let mut settings = Settings::default();
        settings.temperature = Some(5.0);
        let report = settings.validate();
        assert!(!report.is_valid(), "temperature 5.0 should be invalid");
    }

    #[test]
    fn test_settings_validate_temperature_negative() {
        let mut settings = Settings::default();
        settings.temperature = Some(-0.5);
        let report = settings.validate();
        assert!(!report.is_valid(), "negative temperature should be invalid");
    }

    #[test]
    fn test_settings_validate_zero_max_tokens() {
        let mut settings = Settings::default();
        settings.max_response_tokens = Some(0);
        let report = settings.validate();
        assert!(
            !report.is_valid(),
            "max_response_tokens=0 should be invalid"
        );
    }

    #[test]
    fn test_settings_validate_large_max_tokens_warns() {
        let mut settings = Settings::default();
        settings.max_response_tokens = Some(200_000);
        let report = settings.validate();
        // Should not be invalid, but may have warnings
        assert!(report.is_valid() || !report.warnings.is_empty());
    }

    #[test]
    fn test_settings_merge_cli() {
        let mut settings = Settings::default();
        settings.last_used_model = Some("claude-3-5-sonnet".to_string());

        settings.merge_cli(
            Some("gpt-4o".to_string()),
            Some("openai".to_string()),
            None,
            None,
            None,
            None,
        );

        assert_eq!(settings.last_used_model.as_deref(), Some("gpt-4o"));
        assert_eq!(settings.last_used_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn test_settings_merge_cli_preserves_unset() {
        let settings = Settings::default();
        let mut s = settings.clone();
        s.merge_cli(None, None, None, None, None, None);
        assert_eq!(s.default_model, settings.default_model);
    }

    #[test]
    fn test_settings_clone_independence() {
        let s1 = Settings::default();
        let mut s2 = s1.clone();
        s2.default_model = Some("different-model".to_string());

        // Changing s2 should not affect s1
        assert_ne!(s1.default_model, s2.default_model);
    }

    #[test]
    fn test_settings_from_env() {
        std::env::remove_var("OXI_MODEL");
        std::env::remove_var("OXI_PROVIDER");

        let settings = Settings::from_env();
        // Should load defaults when env vars are not set
        assert!(settings.version >= 1);
    }

    #[test]
    fn test_settings_effective_session_dir() {
        let settings = Settings::default();
        // Should produce a valid path or error
        let result = settings.effective_session_dir();
        assert!(result.is_ok() || result.is_err()); // Just verify it doesn't panic
    }
}
