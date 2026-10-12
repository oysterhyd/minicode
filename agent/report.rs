use crate::models::{Block, Event, Message};
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock};

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
fn plain(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => value.to_string(),
    }
}
fn display(value: &Value) -> String {
    escape(&plain(value))
}
fn timestamp(text: &str) -> String {
    text.replacen('T', " ", 1).chars().take(19).collect()
}
fn clip(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut out = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        out.push('…')
    }
    out
}
fn first<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .find_map(|key| value.get(key).filter(|v| !v.is_null()))
        .unwrap_or(&Value::Null)
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
fn kv(key: &str, value: &str) -> String {
    format!(
        "<div class=\"kv\"><span class=\"k\">{}</span><span>{value}</span></div>",
        escape(key)
    )
}
fn code(value: &Value) -> String {
    format!("<code>{}</code>", display(value))
}
fn add(rows: &mut Vec<String>, key: &str, value: &Value, monospace: bool) {
    if truthy(value) {
        rows.push(kv(
            key,
            &if monospace {
                code(value)
            } else {
                display(value)
            },
        ))
    }
}
fn pass(value: &Value) -> &'static str {
    if value.is_null() {
        "<span class=\"muted\">未知</span>"
    } else if truthy(value) {
        "<span class=\"pass\">✓ 通过</span>"
    } else {
        "<span class=\"notpass\">✗ 失败</span>"
    }
}
fn json(value: &Value, pretty: bool) -> String {
    if pretty {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    }
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut escaped = false;
    for c in raw.chars() {
        out.push(c);
        if quoted {
            if escaped {
                escaped = false
            } else if c == '\\' {
                escaped = true
            } else if c == '"' {
                quoted = false
            }
        } else if c == '"' {
            quoted = true
        } else if c == ',' || c == ':' {
            out.push(' ')
        }
    }
    out
}
fn goal_items(data: &Value) -> Vec<&Value> {
    if let Some(items) = data["items"].as_array().filter(|a| !a.is_empty()) {
        items.iter().collect()
    } else if [
        "item",
        "name",
        "check",
        "description",
        "passed",
        "ok",
        "success",
    ]
    .iter()
    .any(|k| data.get(k).is_some())
    {
        vec![data]
    } else {
        vec![]
    }
}
fn usage(data: &Value) -> String {
    let zero = Value::from(0);
    format!(
        "输入 {} · 输出 {} · 缓存读 {} · 缓存写 {}",
        display(data.get("input_tokens").unwrap_or(&zero)),
        display(data.get("output_tokens").unwrap_or(&zero)),
        display(data.get("cache_read_tokens").unwrap_or(&zero)),
        display(data.get("cache_write_tokens").unwrap_or(&zero))
    )
}
fn source(rows: &mut Vec<String>, data: &Value) {
    let s = &data["source"];
    if truthy(&s["plugin"]) {
        rows.push(kv(
            "来源",
            &format!(
                "<code>{}@{} sha256:{}</code>",
                display(&s["plugin"]),
                display(&s["plugin_version"]),
                escape(
                    &plain(
                        s.get("plugin_sha256")
                            .unwrap_or(&Value::String(String::new()))
                    )
                    .chars()
                    .take(12)
                    .collect::<String>()
                )
            ),
        ))
    }
}
fn diff(content: &str) -> String {
    content
        .split('\n')
        .map(|line| {
            let class = if line.starts_with("@@") {
                "d-hunk"
            } else if line.starts_with("+++") || line.starts_with("---") {
                "d-meta"
            } else if line.starts_with('+') {
                "d-add"
            } else if line.starts_with('-') {
                "d-del"
            } else {
                ""
            };
            if class.is_empty() {
                escape(line)
            } else {
                format!("<span class=\"{class}\">{}</span>", escape(line))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn body(
    event: &Event,
    full: &HashMap<String, String>,
    results: &HashMap<&str, &str>,
    names: &HashMap<&str, &str>,
    labels: &Value,
) -> String {
    let data = &event.data;
    let mut rows = vec![];
    match event.r#type.as_str() {
        "session_start" => {
            add(&mut rows, "工作区", &data["workspace"], true);
            add(&mut rows, "提供商", &data["provider"], false);
            add(&mut rows, "模型", &data["model"], false);
        }
        "round_start" | "round_end" => {
            if !data["round"].is_null() {
                rows.push(kv("轮次", &format!("第 {} 轮", display(&data["round"]))))
            }
        }
        "assistant_message" => {
            if let Some(text) = data["text"].as_str().filter(|s| !s.is_empty()) {
                rows.push(format!("<div class=\"msg-text\">{}</div>", escape(text)))
            }
            if let Some(calls) = data["tool_calls"].as_array().filter(|a| !a.is_empty()) {
                rows.push(kv("工具调用", &calls.iter().map(code).collect::<String>()))
            }
            if data["usage"].is_object() {
                rows.push(kv(
                    "Token",
                    &if data["usage"]["available"] == false {
                        "未知（provider 未返回 usage）".into()
                    } else {
                        usage(&data["usage"])
                    },
                ))
            }
            add(&mut rows, "停止原因", &data["stop_reason"], false);
        }
        "tool_call_start" => {
            add(&mut rows, "工具", &data["name"], true);
            source(&mut rows, data);
            let empty = serde_json::json!({});
            let args = data
                .get("arguments")
                .filter(|v| !v.is_null())
                .unwrap_or(&empty);
            let text = json(args, false);
            if text != "{}" {
                rows.push(kv(
                    "参数",
                    &format!("<code>{}</code>", escape(&clip(&text, 200))),
                ));
                if text.chars().count() > 200 {
                    rows.push(format!(
                        "<details><summary>展开完整参数</summary><pre>{}</pre></details>",
                        escape(&json(args, true))
                    ))
                }
            }
        }
        "tool_call_result" => {
            rows.push(kv(
                "结果",
                if data["success"].is_null() || truthy(&data["success"]) {
                    "<span class=\"pass\">✓ 成功</span>"
                } else {
                    "<span class=\"notpass\">✗ 失败</span>"
                },
            ));
            let id = data["call_id"].as_str().unwrap_or_default();
            let name = data["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| names.get(id).copied())
                .unwrap_or_default();
            if !name.is_empty() {
                rows.push(kv("工具", &format!("<code>{}</code>", escape(name))))
            }
            source(&mut rows, data);
            if !data["exit_code"].is_null() {
                rows.push(kv("退出码", &code(&data["exit_code"])))
            }
            if let Some(error) = data["error"].as_str().filter(|s| !s.is_empty()) {
                rows.push(kv(
                    "错误",
                    &format!("<code class=\"err\">{}</code>", escape(error)),
                ))
            }
            let preview = data["output_preview"].as_str().unwrap_or_default();
            if !preview.is_empty() {
                rows.push(format!("<pre class=\"preview\">{}</pre>", escape(preview)))
            }
            let output = full
                .get(id)
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    if ["edit", "write"].contains(&name) {
                        results.get(id).copied()
                    } else {
                        None
                    }
                });
            if let Some(text) = output.filter(|s| !s.is_empty() && s.trim() != preview.trim()) {
                let head = text.lines().take(50).collect::<Vec<_>>();
                let is_diff = ["edit", "write"].contains(&name)
                    || (head.iter().any(|l| l.starts_with("--- "))
                        && head.iter().any(|l| l.starts_with("+++ ")))
                    || head.iter().any(|l| l.starts_with("diff --git"));
                let (label, class, inner) = if is_diff {
                    ("展开完整 diff", "diff", diff(text))
                } else {
                    ("展开完整输出", "full", escape(text))
                };
                rows.push(format!("<details><summary>{label}</summary><pre class=\"{class}\">{inner}</pre></details>"));
            }
        }
        "approval_request" => {
            add(&mut rows, "工具", first(data, &["tool_name", "tool"]), true);
            add(&mut rows, "内容", &data["summary"], false);
        }
        "approval_decision" => {
            rows.push(kv(
                "决定",
                if truthy(&data["granted"]) {
                    "<span class=\"pass\">✓ 允许</span>"
                } else {
                    "<span class=\"notpass\">✗ 拒绝</span>"
                },
            ));
            add(&mut rows, "工具", first(data, &["tool_name", "tool"]), true);
            add(&mut rows, "原因", &data["reason"], false);
        }
        "session_end" => {
            if let Some(reason) = data["exit_reason"].as_str().filter(|s| !s.is_empty()) {
                let label = labels["exit"][reason].as_str().unwrap_or(reason);
                rows.push(kv(
                    "退出原因",
                    &format!("{} <code>{}</code>", escape(label), escape(reason)),
                ))
            }
            if !data["rounds"].is_null() {
                rows.push(kv("轮数", &display(&data["rounds"])))
            }
            if data["total_usage"].is_object() {
                rows.push(kv(
                    "Token",
                    &format!(
                        "{}{}",
                        usage(&data["total_usage"]),
                        if data["total_usage"]["available"] == false {
                            " · 用量不完整"
                        } else {
                            ""
                        }
                    ),
                ))
            }
            if let Some(error) = data["error"].as_str().filter(|s| !s.is_empty()) {
                rows.push(kv(
                    "错误",
                    &format!("<code class=\"err\">{}</code>", escape(error)),
                ))
            }
        }
        "context_compacted" => {
            let before = first(data, &["before_tokens", "tokens_before", "before"]);
            let after = first(data, &["after_tokens", "tokens_after", "after"]);
            if !before.is_null() || !after.is_null() {
                rows.push(kv(
                    "Token",
                    &format!("{} → {}", display(before), display(after)),
                ))
            }
        }
        "goal_check" => {
            for item in goal_items(data) {
                let mut bits = vec![pass(first(item, &["passed", "ok", "success"])).to_string()];
                let text = first(item, &["item", "name", "check", "description"]);
                if truthy(text) {
                    bits.push(format!("<span class=\"mono\">{}</span>", display(text)))
                }
                if truthy(&item["kind"]) {
                    bits.push(format!(
                        "<span class=\"muted\">kind={}</span>",
                        display(&item["kind"])
                    ))
                }
                if !item["exit_code"].is_null() {
                    bits.push(format!("退出码 {}", code(&item["exit_code"])))
                }
                rows.push(format!("<div class=\"kv\">{}</div>", bits.join(" ")));
            }
        }
        "side_effect_unknown" => {
            add(
                &mut rows,
                "工具",
                first(data, &["tool", "tool_name", "name"]),
                true,
            );
            add(
                &mut rows,
                "说明",
                first(data, &["detail", "description", "reason", "error"]),
                false,
            );
            rows.push(kv("状态", "<span class=\"muted\">副作用结果未知</span>"));
        }
        "background_job_started" => {
            add(
                &mut rows,
                "任务",
                first(data, &["job_id", "job", "id"]),
                true,
            );
            if let Some(command) = first(data, &["command", "cmd", "name", "description"])
                .as_str()
                .filter(|s| !s.is_empty())
            {
                rows.push(kv("内容", &escape(&clip(command, 200))))
            }
        }
        "background_job_completed" => {
            add(
                &mut rows,
                "任务",
                first(data, &["job_id", "job", "id"]),
                true,
            );
            if !data["exit_code"].is_null() {
                rows.push(kv("退出码", &code(&data["exit_code"])))
            }
            if let Some(text) = first(data, &["output", "output_preview"])
                .as_str()
                .filter(|s| !s.is_empty())
            {
                rows.push(format!(
                    "<pre class=\"preview\">{}</pre>",
                    escape(&clip(text, 400))
                ))
            }
        }
        "background_job_lost" => {
            let job = first(data, &["job_id", "job", "id"]);
            if truthy(job) {
                rows.push(kv("任务", &format!("{} 结果丢失", code(job))))
            }
        }
        _ => {}
    }
    if rows.is_empty() {
        if truthy(data) {
            format!(
                "<pre class=\"raw\">{}</pre>",
                escape(&clip(&json(data, true), 1200))
            )
        } else {
            "<span class=\"muted\">（无数据）</span>".into()
        }
    } else {
        rows.join("\n")
    }
}

pub fn render(
    summary: &Value,
    events: &[Event],
    messages: &[Message],
    full: &HashMap<String, String>,
) -> String {
    static LABELS: OnceLock<Value> = OnceLock::new();
    let labels = LABELS
        .get_or_init(|| serde_json::from_str(include_str!("assets/report-labels.json")).unwrap());
    let mut results = HashMap::new();
    for block in messages.iter().flat_map(|m| &m.content) {
        if let Block::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
            && !content.is_empty()
        {
            results
                .entry(tool_use_id.as_str())
                .or_insert(content.as_str());
        }
    }
    let mut names = HashMap::new();
    for event in events {
        if let (Some(id), Some(name)) = (
            event.data["call_id"].as_str(),
            event.data["name"].as_str().filter(|s| !s.is_empty()),
        ) {
            names.entry(id).or_insert(name);
        }
    }
    let mut ordered = events.iter().collect::<Vec<_>>();
    if ordered.windows(2).any(|w| w[0].seq > w[1].seq) {
        ordered.sort_by_key(|e| e.seq)
    }
    let id = display(&summary["session_id"]);
    let status = summary["status"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("running");
    let label = labels["exit"][status]
        .as_str()
        .or(labels["status"][status].as_str())
        .unwrap_or(status);
    let mut status_html = format!(
        "{} <span class=\"muted\">({})</span>",
        escape(label),
        escape(status)
    );
    if let Some(reason) = summary["exit_reason"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != status)
    {
        status_html.push_str(&format!(
            " <span class=\"muted\">· 退出原因：{} ({})</span>",
            escape(labels["exit"][reason].as_str().unwrap_or(reason)),
            escape(reason)
        ))
    }
    let input = summary["input_tokens"].as_u64().unwrap_or(0);
    let output = summary["output_tokens"].as_u64().unwrap_or(0);
    let total = input.saturating_add(output);
    let header_rows = [
        ("会话 ID", format!("<code>{id}</code>")),
        ("状态", status_html),
        ("工作区", code(&summary["workspace"])),
        (
            "模型",
            format!(
                "{} / {}",
                display(&summary["provider"]),
                display(&summary["model"])
            ),
        ),
        (
            "创建时间",
            escape(&timestamp(
                summary["created_at"].as_str().unwrap_or_default(),
            )),
        ),
        ("轮数", display(&summary["rounds"])),
        (
            "Token",
            format!("输入 {input} · 输出 {output} · 总计 {total}"),
        ),
        (
            "缓存 Token",
            format!(
                "读取 {} · 写入 {}{}",
                display(&summary["cache_read_tokens"]),
                display(&summary["cache_write_tokens"]),
                if summary["usage_available"] == false {
                    " · 用量不完整"
                } else {
                    ""
                }
            ),
        ),
    ];
    let header = format!(
        "<header class=\"panel\"><h1>Mini Claude Code 执行报告</h1><table class=\"meta\">{}</table></header>",
        header_rows
            .iter()
            .map(|(k, v)| format!("<tr><th>{k}</th><td>{v}</td></tr>"))
            .collect::<String>()
    );
    let dash = "<span class=\"muted\">—</span>";
    let mut goal_rows = vec![];
    for event in ordered.iter().filter(|e| e.r#type == "goal_check") {
        let mut items = goal_items(&event.data);
        if items.is_empty() {
            items.push(&Value::Null)
        }
        for item in items {
            let text = first(item, &["item", "name", "check", "description"]);
            goal_rows.push(format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"ev-time\">{}</td></tr>",if truthy(text){display(text)}else{dash.into()},if truthy(&item["kind"]){display(&item["kind"])}else{dash.into()},pass(first(item,&["passed","ok","success"])),if item["exit_code"].is_null(){dash.into()}else{code(&item["exit_code"])},escape(&timestamp(&event.timestamp))));
        }
    }
    let goals = if goal_rows.is_empty() {
        "<p class=\"muted\">（无 goal_check 事件）</p>".into()
    } else {
        format!(
            "<table class=\"goal-table\"><thead><tr><th>item</th><th>kind</th><th>通过</th><th>退出码</th><th>时间</th></tr></thead><tbody>{}</tbody></table>",
            goal_rows.join("\n")
        )
    };
    let timeline = if ordered.is_empty() {
        "<p class=\"muted\">（无事件记录）</p>".into()
    } else {
        let rows=ordered.iter().map(|event|{let meta=&labels["events"][&event.r#type];let label=meta[0].as_str().unwrap_or(&event.r#type);let mut category=meta[1].as_str().unwrap_or("round");let mut state="";if ["tool_call_result","approval_decision"].contains(&event.r#type.as_str()){let ok=if event.r#type=="tool_call_result"{event.data["success"].is_null()||truthy(&event.data["success"])}else{truthy(&event.data["granted"])};category=if ok{"ok"}else{"fail"};state=category;}format!("<div class=\"event {state}\">\n<div class=\"ev-head\"><span class=\"badge b-{category}\">{}</span><code class=\"ev-type\">{}</code><span class=\"ev-time\">#{} · {}</span></div>\n<div class=\"ev-body\">{}</div>\n</div>",escape(label),escape(&event.r#type),event.seq,escape(&timestamp(&event.timestamp)),body(event,full,&results,&names,labels))}).collect::<Vec<_>>();
        format!("<div class=\"timeline\">{}</div>", rows.join("\n"))
    };
    let footer = format!(
        "<footer class=\"panel\"><h2>用量</h2><p>输入 Token <strong>{input}</strong> · 输出 Token <strong>{output}</strong> · 总计 <strong>{total}</strong> · 缓存读取 <strong>{}</strong> · 缓存写入 <strong>{}</strong> · 轮数 <strong>{}</strong></p><p class=\"muted\">单文件离线报告：无外部 CSS / JS / 字体依赖，无需 JavaScript 即可查看全部内容（折叠区使用原生 details 元素）。</p></footer>",
        display(&summary["cache_read_tokens"]),
        display(&summary["cache_write_tokens"]),
        display(&summary["rounds"])
    );
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Mini Claude Code 执行报告 — {id}</title>\n<style>{}</style>\n</head>\n<body>\n<div class=\"wrap\">\n{header}<section class=\"panel\"><h2>目标验收（Goal Checks）</h2>{goals}</section><section class=\"panel\"><h2>时间线</h2>{timeline}</section>{footer}\n</div>\n</body>\n</html>\n",
        include_str!("assets/report.css")
    )
}
