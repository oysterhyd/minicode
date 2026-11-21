use crate::util;
use anyhow::Result;
use serde_json::{Value, json};
use std::{fmt, time::Duration};

/// Provider failures carry their retry semantics independently of their display text.
#[derive(Debug)]
pub enum ProviderError {
    Http {
        status: u16,
        body: String,
        retry_after: Option<Duration>,
    },
    ContextOverflow {
        status: Option<u16>,
        body: String,
    },
    Stream {
        kind: String,
        message: String,
    },
    Network(reqwest::Error),
    RecordedNetwork,
    Timeout {
        idle: Duration,
    },
    Deadline,
    Cancelled,
    Interrupted,
    Protocol(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { status, .. } => {
                *status == 408 || *status == 429 || (500..600).contains(status)
            }
            Self::Network(_) | Self::RecordedNetwork | Self::Timeout { .. } | Self::Interrupted => {
                true
            }
            Self::Stream { kind, .. } => matches!(
                kind.as_str(),
                "overloaded"
                    | "overloaded_error"
                    | "rate_limit_error"
                    | "rate_limit_exceeded"
                    | "api_error"
                    | "server_error"
                    | "internal_server_error"
            ),
            Self::Protocol(_) | Self::ContextOverflow { .. } | Self::Deadline | Self::Cancelled => {
                false
            }
        }
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { status, body, .. } => {
                write!(f, "provider HTTP {status}")?;
                if !body.is_empty() {
                    write!(f, ": {body}")?;
                }
                if *status == 401 {
                    write!(f, " (请检查并重新配置 API 凭据)")?;
                }
                Ok(())
            }
            Self::ContextOverflow { status, body } => {
                if let Some(status) = status {
                    write!(f, "provider HTTP {status}: {body}")
                } else {
                    write!(f, "provider context overflow: {body}")
                }
            }
            Self::Stream { kind, message } => {
                write!(f, "provider stream error ({kind}): {message}")
            }
            Self::Network(_) | Self::RecordedNetwork => write!(f, "provider connection failed"),
            Self::Timeout { idle } => write!(
                f,
                "provider stream idle timeout: no data for {}s",
                idle.as_secs_f64()
            ),
            Self::Interrupted => write!(f, "provider stream ended without a final response"),
            Self::Deadline => write!(f, "turn deadline exceeded"),
            Self::Cancelled => write!(f, "turn cancelled"),
            Self::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(source) => Some(source),
            _ => None,
        }
    }
}

pub(super) fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let deadline = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        (deadline.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}

pub(super) fn context_overflow(body: &str) -> bool {
    let error = match serde_json::from_str::<Value>(body) {
        Ok(value) => value["error"].clone(),
        Err(_) => {
            // The response may be truncated after a complete error object.
            let Some(rest) = body
                .split_once("\"error\"")
                .map(|(_, rest)| rest.trim_start())
            else {
                return false;
            };
            let Some(rest) = rest.strip_prefix(':') else {
                return false;
            };
            let Some(Ok(error)) = serde_json::Deserializer::from_str(rest.trim_start())
                .into_iter::<Value>()
                .next()
            else {
                return false;
            };
            error
        }
    };
    let code = error["code"]
        .as_str()
        .or_else(|| error["type"].as_str())
        .unwrap_or_default();
    if matches!(
        code,
        "context_length_exceeded"
            | "context_window_exceeded"
            | "context_limit_exceeded"
            | "prompt_too_long"
            | "input_too_long"
    ) {
        return true;
    }
    let message = error["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    message.contains("maximum context length")
        || message.contains("prompt is too long")
        || message.contains("input is too long")
}

pub(super) fn stream_error(value: &Value) -> ProviderError {
    let error = value
        .get("error")
        .filter(|error| !error.is_null())
        .unwrap_or(value);
    let kind = error["type"]
        .as_str()
        .or_else(|| error["code"].as_str())
        .unwrap_or("unknown");
    let message = error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .unwrap_or("provider returned an unspecified error");
    let kind = util::prefix(kind, 128).to_string();
    let message = util::prefix(message, 4096).to_string();
    if context_overflow(&json!({"error":error}).to_string()) {
        ProviderError::ContextOverflow {
            status: None,
            body: format!("{kind}: {message}"),
        }
    } else {
        ProviderError::Stream { kind, message }
    }
}

/// Longest silence tolerated while waiting for response headers or the next
/// body chunk. MINICODE_PROVIDER_IDLE_TIMEOUT_S overrides the default.
pub(super) fn idle_timeout() -> Duration {
    std::env::var("MINICODE_PROVIDER_IDLE_TIMEOUT_S")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .unwrap_or(Duration::from_secs(120))
}
pub(super) async fn idle<F: std::future::Future>(limit: Duration, future: F) -> Result<F::Output> {
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| ProviderError::Timeout { idle: limit }.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("5"), Some(Duration::from_secs(5)));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(Duration::ZERO)
        );
        assert!(
            parse_retry_after("Wed, 21 Oct 2099 07:28:00 GMT").unwrap() > Duration::from_secs(60)
        );
        assert_eq!(parse_retry_after("not a date"), None);
    }

    #[test]
    fn context_overflow_uses_known_codes_and_messages() {
        assert!(context_overflow(
            r#"{"error":{"code":"context_length_exceeded"}}"#
        ));
        assert!(context_overflow(
            r#"{"error":{"code":"context_length_exceeded"},"padding":"truncated"#
        ));
        assert!(context_overflow(
            r#"{"error":{"type":"invalid_request_error","message":"prompt is too long"}}"#
        ));
        assert!(!context_overflow(
            r#"{"error":{"type":"invalid_request_error","message":"bad parameter"}}"#
        ));
        assert!(!context_overflow("context_length_exceeded"));
    }

    #[test]
    fn stream_error_classification_preserves_kind_and_message() {
        for kind in ["overloaded_error", "rate_limit_error", "api_error"] {
            let error = stream_error(&json!({"error":{"type":kind,"message":"please retry"}}));
            assert!(error.is_retryable(), "{kind}");
            assert!(error.to_string().contains(kind));
            assert!(error.to_string().contains("please retry"));
        }
        let invalid = stream_error(
            &json!({"error":{"type":"invalid_request_error","message":"bad parameter"}}),
        );
        assert!(!invalid.is_retryable());
        let overflow = stream_error(
            &json!({"error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"too long"}}),
        );
        assert!(matches!(
            overflow,
            ProviderError::ContextOverflow { status: None, .. }
        ));
        assert!(!overflow.is_retryable());
    }
}
