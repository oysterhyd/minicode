use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default)]
        source: String,
    },
    RedactedThinking {
        data: String,
        #[serde(default)]
        source: String,
    },
    Media {
        asset_id: String,
        kind: String,
        name: String,
        mime_type: String,
        size: u64,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_error: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        images: Vec<Block>,
    },
}
impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Vec<Block>,
}
impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: vec![Block::text(text)],
        }
    }
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| {
                if let Block::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    #[serde(default = "legacy_usage_available")]
    pub available: bool,
}
fn legacy_usage_available() -> bool {
    true
}
impl Usage {
    pub fn total(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
    pub fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.available |= other.available;
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub max_rounds: u64,
    pub max_total_tokens: i64,
    pub max_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub r#type: String,
    pub timestamp: String,
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub requires_approval: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub success: bool,
    pub output: String,
    #[serde(default)]
    pub images: Vec<Block>,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub job_id: Option<String>,
    #[serde(skip)]
    pub full_output: Option<String>,
}
impl Outcome {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            images: vec![],
            error: None,
            exit_code: None,
            job_id: None,
            full_output: None,
        }
    }
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(error.into()),
            ..Self::ok("")
        }
    }
}
#[derive(Clone, Debug)]
pub struct Response {
    pub blocks: Vec<Block>,
    pub stop_reason: String,
    pub usage: Usage,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub session_id: String,
    pub exit_reason: String,
    pub rounds: u64,
    pub total_usage: Usage,
    pub duration_s: f64,
    pub error: Option<String>,
}
