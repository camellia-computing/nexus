//! Bounded, value-free summaries of untrusted program check output.

use serde::{Deserialize, Serialize};

use crate::CommandOutput;

const MAX_CLASSIFICATION_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeDiagnosticReport {
    pub message_key: String,
    pub exit_code: Option<i32>,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

impl NativeDiagnosticReport {
    pub fn from_output(output: &CommandOutput) -> Self {
        let message_key = if output.success {
            "CORE_CHECK_COMPLETED"
        } else {
            classify(&output.stdout, &output.stderr)
        };
        Self {
            message_key: message_key.into(),
            exit_code: output.code,
            stdout_bytes: output.stdout.len(),
            stderr_bytes: output.stderr.len(),
        }
    }
}

fn classification_prefix(value: &str) -> String {
    let mut end = value.len().min(MAX_CLASSIFICATION_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_ascii_lowercase()
}

fn classify(stdout: &str, stderr: &str) -> &'static str {
    let streams = [classification_prefix(stdout), classification_prefix(stderr)];
    // Match diagnostic categories, never retain the values or paths following a native message.
    let categories: &[(&str, &[&str])] = &[
        (
            "CORE_NATIVE_FIELD_REJECTED",
            &["json: unknown field", "unexpected key:"],
        ),
        (
            "CORE_NATIVE_TYPE_REJECTED",
            &["json: cannot unmarshal", "yaml: unmarshal errors"],
        ),
        (
            "CORE_NATIVE_SYNTAX_REJECTED",
            &["unexpected end of json input", "invalid character "],
        ),
        (
            "CORE_NATIVE_PORT_REJECTED",
            &[
                "invalid port:",
                "invalid port range:",
                "invalid port-range format",
            ],
        ),
        (
            "CORE_NATIVE_RESOURCE_UNAVAILABLE",
            &["no such file or directory", "permission denied"],
        ),
    ];
    let mut selected = None;
    for &(category, patterns) in categories {
        if streams
            .iter()
            .any(|stream| patterns.iter().any(|pattern| stream.contains(pattern)))
        {
            if selected.is_some() {
                return "CORE_NATIVE_REJECTED";
            }
            selected = Some(category);
        }
    }
    selected.unwrap_or("CORE_NATIVE_REJECTED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(message: &str) -> CommandOutput {
        CommandOutput {
            code: Some(1),
            success: false,
            stdout: String::new(),
            stderr: message.into(),
        }
    }

    #[test]
    fn report_contains_only_fixed_categories_and_process_metadata() {
        for (message, key) in [
            (
                "json: unknown field \"private-token\"",
                "CORE_NATIVE_FIELD_REJECTED",
            ),
            (
                "json: cannot unmarshal private-token into Go struct field",
                "CORE_NATIVE_TYPE_REJECTED",
            ),
            ("invalid port: private-token", "CORE_NATIVE_PORT_REJECTED"),
            (
                "private-token: permission denied",
                "CORE_NATIVE_RESOURCE_UNAVAILABLE",
            ),
            (
                "unexpected end of JSON input: private-token",
                "CORE_NATIVE_SYNTAX_REJECTED",
            ),
            (
                "unrecognized failure: private-token",
                "CORE_NATIVE_REJECTED",
            ),
            (
                "json: cannot unmarshal value; invalid port: private-token",
                "CORE_NATIVE_REJECTED",
            ),
        ] {
            let report = NativeDiagnosticReport::from_output(&output(message));
            assert_eq!(report.message_key, key);
            assert_eq!(report.stderr_bytes, message.len());
            assert_eq!(report.exit_code, Some(1));
            assert!(
                !serde_json::to_string(&report)
                    .unwrap()
                    .contains("private-token")
            );
        }
    }

    #[test]
    fn arbitrary_encodings_control_characters_and_config_echoes_cannot_escape_the_report() {
        let sensitive = concat!(
            "https://private-user:private-password@private.example/private-token\n",
            "-----BEGIN ",
            "PRIVATE KEY-----\nprivate-key-material\n-----END PRIVATE KEY-----\n",
            "{\"uuid\":\"32e48a9f-0326-4f37-92fe-ad489fe055f6\"}\n\u{1b}[31m\u{202e}cHJpdmF0ZS10b2tlbg==",
        );
        for success in [false, true] {
            let result = CommandOutput {
                code: Some(i32::from(!success)),
                success,
                stdout: sensitive.into(),
                stderr: sensitive.into(),
            };
            let report = NativeDiagnosticReport::from_output(&result);
            let encoded = serde_json::to_value(&report).unwrap();
            assert_eq!(encoded.as_object().unwrap().len(), 4);
            assert_eq!(
                report.message_key,
                if success {
                    "CORE_CHECK_COMPLETED"
                } else {
                    "CORE_NATIVE_REJECTED"
                }
            );
            assert!(!encoded.to_string().contains("private"));
            assert!(!encoded.to_string().contains("32e48a9f"));
            assert!(!encoded.to_string().contains("cHJpdmF0"));
        }
    }

    #[test]
    fn classification_is_bounded_and_success_does_not_become_a_rejection() {
        let message = format!(
            "{}json: unknown field secret",
            "测".repeat(MAX_CLASSIFICATION_BYTES)
        );
        let report = NativeDiagnosticReport::from_output(&output(&message));
        assert_eq!(report.message_key, "CORE_NATIVE_REJECTED");
        assert!(serde_json::to_string(&report).unwrap().len() < 180);
        let mut accepted = output("json: unknown field private-token");
        accepted.success = true;
        accepted.code = Some(0);
        assert_eq!(
            NativeDiagnosticReport::from_output(&accepted).message_key,
            "CORE_CHECK_COMPLETED"
        );
    }
}
