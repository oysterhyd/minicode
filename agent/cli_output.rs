use crate::util;
use serde_json::Value;

pub fn tool_output(data: &Value, verbose: bool) -> String {
    let raw = util::terminal_text(
        data["output_detail"]
            .as_str()
            .or_else(|| data["output_preview"].as_str())
            .unwrap_or_default(),
    );
    let mut output = if verbose {
        raw.clone()
    } else {
        util::prefix(&raw.lines().take(10).collect::<Vec<_>>().join("\n"), 2000).to_string()
    };
    if !verbose && output.len() < raw.len() {
        output.push_str("\n… 输出已截断；-v / --verbose 查看完整输出");
    }
    if let Some(error) = data["error"].as_str().filter(|error| !error.is_empty()) {
        let error = util::terminal_text(error);
        if !output.contains(&error) {
            output.push('\n');
            output.push_str(&error);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn l7_output_is_bounded_and_errors_survive() {
        let detail = (0..1000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let data = json!({"output_detail":detail,"error":"critical end error"});
        let preview = tool_output(&data, false);
        assert!(preview.len() < 2500);
        assert!(!preview.contains("line 999"));
        assert!(preview.contains("critical end error"));
        assert!(tool_output(&data, true).contains("line 999"));
    }
}
