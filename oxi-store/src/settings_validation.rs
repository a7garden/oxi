//! 설정 검증 모듈.
//!
//! 애플리케이션 시작 시 모든 설정 값을 검증하여
//! 런타임 패닉을 사전에 방지한다.

use crate::settings::Settings;

/// 검증 결과
#[derive(Debug)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ValidationWarning {
    pub field: String,
    pub message: String,
}

impl ValidationReport {
    /// 에러가 없으면 `true`를 반환한다.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

impl Settings {
    /// 현재 설정의 유효성을 검증한다.
    ///
    /// 에러는 즉시 프로그램 종료 사유가 되며, 경고는 로그에 남기만 한다.
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport {
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        // 1. default_temperature — 0.0~2.0 범위
        if let Some(temp) = self.default_temperature {
            if !(0.0..=2.0).contains(&temp) {
                report.errors.push(ValidationError {
                    field: "default_temperature".to_string(),
                    message: format!(
                        "Temperature must be between 0.0 and 2.0 (current: {})",
                        temp
                    ),
                });
            }
        }
        // 레거시 temperature(f32) 필드도 검증
        if let Some(temp) = self.temperature {
            if !(0.0..=2.0).contains(&temp) {
                report.errors.push(ValidationError {
                    field: "temperature".to_string(),
                    message: format!(
                        "Temperature must be between 0.0 and 2.0 (current: {})",
                        temp
                    ),
                });
            }
        }

        // 2. max_response_tokens — 최소 1, 128000 초과 시 경고
        if let Some(tokens) = self.max_response_tokens {
            if tokens == 0 {
                report.errors.push(ValidationError {
                    field: "max_response_tokens".to_string(),
                    message: "Must be at least 1 (current: 0)".to_string(),
                });
            } else if tokens > 128_000 {
                report.warnings.push(ValidationWarning {
                    field: "max_response_tokens".to_string(),
                    message: format!(
                        "Value exceeds 128,000. Most models may not support this (current: {})",
                        tokens
                    ),
                });
            }
        }
        // 레거시 max_tokens(u32)도 동일 검증
        if let Some(tokens) = self.max_tokens {
            if tokens == 0 {
                report.errors.push(ValidationError {
                    field: "max_tokens".to_string(),
                    message: "Must be at least 1 (current: 0)".to_string(),
                });
            } else if tokens as usize > 128_000 {
                report.warnings.push(ValidationWarning {
                    field: "max_tokens".to_string(),
                    message: format!(
                        "Value exceeds 128,000. Most models may not support this (current: {})",
                        tokens
                    ),
                });
            }
        }

        // 3. tool_timeout_seconds — 최소 1
        if self.tool_timeout_seconds == 0 {
            report.errors.push(ValidationError {
                field: "tool_timeout_seconds".to_string(),
                message: "Must be at least 1 second (current: 0)".to_string(),
            });
        }

        // 4. thinking_level — 열거형이므로 직렬화/역직렬화 단계에서 이미 검증됨.
        //    하지만 env 등을 통해 우회 입력된 값도 있으니 안전망으로 확인.
        //    (ThinkingLevel은 이미 enum이므로 유효하지 않은 값은 역직렬화에서 거부됨)

        // 5. default_model — model name only (no provider prefix expected)
        // No validation needed: model name may or may not contain '/' depending on user input.

        // 6. session_history_size — 최소 1
        if self.session_history_size == 0 {
            report.errors.push(ValidationError {
                field: "session_history_size".to_string(),
                message: "Must be at least 1 (current: 0)".to_string(),
            });
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ThinkingLevel;

    /// 기본 설정은 에러·경고 모두 없어야 한다.
    #[test]
    fn test_default_settings_are_valid() {
        let settings = Settings::default();
        let report = settings.validate();
        assert!(report.is_valid(), "default settings should be valid");
        assert!(
            report.warnings.is_empty(),
            "default settings should have no warnings"
        );
    }

    // ── default_temperature ──────────────────────────────────────────

    #[test]
    fn test_temperature_in_range() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(1.5);
        let report = settings.validate();
        assert!(report.is_valid());
    }

    #[test]
    fn test_temperature_at_boundaries() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(0.0);
        assert!(settings.validate().is_valid());

        settings.default_temperature = Some(2.0);
        assert!(settings.validate().is_valid());
    }

    #[test]
    fn test_temperature_below_range() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(-0.1);
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].field, "default_temperature");
    }

    #[test]
    fn test_temperature_above_range() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(2.5);
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].field, "default_temperature");
    }

    // ── legacy temperature (f32) ─────────────────────────────────────

    #[test]
    fn test_legacy_temperature_out_of_range() {
        let mut settings = Settings::default();
        settings.temperature = Some(3.0);
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "temperature");
    }

    // ── max_response_tokens ──────────────────────────────────────────

    #[test]
    fn test_max_response_tokens_zero_is_error() {
        let mut settings = Settings::default();
        settings.max_response_tokens = Some(0);
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "max_response_tokens");
    }

    #[test]
    fn test_max_response_tokens_above_128k_is_warning() {
        let mut settings = Settings::default();
        settings.max_response_tokens = Some(200_000);
        let report = settings.validate();
        assert!(report.is_valid(), "warning should not block");
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].field, "max_response_tokens");
    }

    #[test]
    fn test_max_response_tokens_normal() {
        let mut settings = Settings::default();
        settings.max_response_tokens = Some(4096);
        let report = settings.validate();
        assert!(report.is_valid());
        assert!(report.warnings.is_empty());
    }

    // ── legacy max_tokens (u32) ──────────────────────────────────────

    #[test]
    fn test_max_tokens_zero_is_error() {
        let mut settings = Settings::default();
        settings.max_tokens = Some(0);
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "max_tokens");
    }

    #[test]
    fn test_max_tokens_above_128k_is_warning() {
        let mut settings = Settings::default();
        settings.max_tokens = Some(200_000);
        let report = settings.validate();
        assert!(report.is_valid());
        assert_eq!(report.warnings.len(), 1);
    }

    // ── tool_timeout_seconds ─────────────────────────────────────────

    #[test]
    fn test_tool_timeout_zero_is_error() {
        let mut settings = Settings::default();
        settings.tool_timeout_seconds = 0;
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "tool_timeout_seconds");
    }

    #[test]
    fn test_tool_timeout_positive_is_ok() {
        let mut settings = Settings::default();
        settings.tool_timeout_seconds = 60;
        assert!(settings.validate().is_valid());
    }

    // ── default_model (now model-only, no slash validation) ──────

    #[test]
    fn test_model_without_slash_is_ok() {
        let mut settings = Settings::default();
        settings.default_model = Some("claude-3".to_string());
        let report = settings.validate();
        assert!(report.is_valid());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn test_model_none_produces_no_warning() {
        let settings = Settings::default();
        let report = settings.validate();
        assert!(report.warnings.is_empty());
    }

    // ── session_history_size ─────────────────────────────────────────

    #[test]
    fn test_session_history_size_zero_is_error() {
        let mut settings = Settings::default();
        settings.session_history_size = 0;
        let report = settings.validate();
        assert!(!report.is_valid());
        assert_eq!(report.errors[0].field, "session_history_size");
    }

    #[test]
    fn test_session_history_size_positive_is_ok() {
        let mut settings = Settings::default();
        settings.session_history_size = 50;
        assert!(settings.validate().is_valid());
    }

    // ── multiple issues ──────────────────────────────────────────────

    #[test]
    fn test_multiple_errors_and_warnings() {
        let mut settings = Settings::default();
        settings.default_temperature = Some(5.0);
        settings.tool_timeout_seconds = 0;
        settings.default_model = Some("badmodel".to_string());
        settings.session_history_size = 0;

        let report = settings.validate();
        assert!(!report.is_valid());
        // 3 errors: temperature, tool_timeout, session_history_size
        assert_eq!(report.errors.len(), 3);
        // No warnings (default_model no longer warns for missing slash)
        assert_eq!(report.warnings.len(), 0);
    }

    // ── ValidationReport helpers ─────────────────────────────────────

    #[test]
    fn test_report_is_valid_with_warnings_only() {
        let report = ValidationReport {
            errors: vec![],
            warnings: vec![ValidationWarning {
                field: "x".to_string(),
                message: "soft warning".to_string(),
            }],
        };
        assert!(report.is_valid());
    }

    #[test]
    fn test_report_is_invalid_with_errors() {
        let report = ValidationReport {
            errors: vec![ValidationError {
                field: "x".to_string(),
                message: "hard error".to_string(),
            }],
            warnings: vec![],
        };
        assert!(!report.is_valid());
    }
}
