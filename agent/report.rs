use crate::{
    models::{Block, Event, Message},
    util,
};
use serde_json::Value;
use std::collections::HashMap;
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
fn display(v: &Value) -> String {
    escape(
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                if v.is_null() {
                    String::new()
                } else {
                    v.to_string()
                }
            })
            .as_str(),
    )
}
fn kv(label: &str, value: &str) -> String {
    format!(
        "<div class=\"kv\"><span class=\"k\">{}</span>{value}</div>",
        escape(label)
    )
}
fn details(label: &str, text: &str) -> String {
    format!(
        "<details><summary>{}</summary><pre>{}</pre></details>",
        escape(label),
        escape(text)
    )
}
pub fn render(
    summary: &Value,
    events: &[Event],
    messages: &[Message],
    full: &HashMap<String, String>,
) -> String {
    let labels: Value = serde_json::from_str(include_str!("assets/report-labels.json")).unwrap();
    let id = display(&summary["session_id"]);
    let status = summary["status"].as_str().unwrap_or("running");
    let label = labels["status"][status]
        .as_str()
        .or(labels["exit"][status].as_str())
        .unwrap_or(status);
    let mut rows = vec![
        ("会话 ID", format!("<code>{id}</code>")),
        (
            "状态",
            format!(
                "{} <span class=\"muted\">({})</span>",
                escape(label),
                escape(status)
            ),
        ),
        (
            "工作区",
            format!("<code>{}</code>", display(&summary["workspace"])),
        ),
        (
            "模型",
            format!(
                "{} / {}",
                display(&summary["provider"]),
                display(&summary["model"])
            ),
        ),
        ("创建时间", display(&summary["created_at"])),
        ("轮数", display(&summary["rounds"])),
    ];
    let input = summary["input_tokens"].as_u64().unwrap_or(0);
    let output = summary["output_tokens"].as_u64().unwrap_or(0);
    rows.push((
        "Token",
        format!("输入 {input} · 输出 {output} · 总计 {}", input + output),
    ));
    rows.push((
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
    ));
    let header = format!(
        "<header class=\"panel\"><h1>Mini Claude Code 执行报告</h1><table class=\"meta\">{}</table></header>",
        rows.iter()
            .map(|(k, v)| format!("<tr><th>{k}</th><td>{v}</td></tr>"))
            .collect::<String>()
    );
    let results = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| {
            if let Block::ToolResult {
                tool_use_id,
                content,
                ..
            } = b
            {
                Some((tool_use_id.clone(), content.clone()))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    let mut goal_rows = String::new();
    let mut timeline = String::new();
    for event in events {
        let data = &event.data;
        let meta = &labels["events"][&event.r#type];
        let label = meta[0].as_str().unwrap_or(&event.r#type);
        let mut category = meta[1].as_str().unwrap_or("round");
        let mut state = "";
        if event.r#type == "tool_call_result" || event.r#type == "approval_decision" {
            let success = if event.r#type == "tool_call_result" {
                data["success"] != false
            } else {
                data["granted"] == true
            };
            category = if success { "ok" } else { "fail" };
            state = if success { "ok" } else { "fail" };
        }
        let mut body = String::new();
        match event.r#type.as_str() {
            "assistant_message" => {
                if let Some(text) = data["text"].as_str() {
                    body.push_str(&format!("<pre class=\"assistant\">{}</pre>", escape(text)));
                }
                if let Some(reasoning) = data["reasoning"].as_str().filter(|s| !s.is_empty()) {
                    body.push_str(&details("思考过程", reasoning));
                }
                body.push_str(&kv("停止原因", &display(&data["stop_reason"])));
            }
            "tool_call_start" | "approval_request" => {
                body.push_str(&kv(
                    "工具",
                    &format!("<code>{}</code>", display(&data["name"])),
                ));
                body.push_str(&details(
                    "参数",
                    &serde_json::to_string_pretty(&data["arguments"]).unwrap_or_default(),
                ));
            }
            "tool_call_result" => {
                body.push_str(&kv("工具", &display(&data["name"])));
                body.push_str(&kv(
                    "结果",
                    if data["success"] == true {
                        "<span class=\"ok\">✓ 通过</span>"
                    } else {
                        "<span class=\"err\">✗ 失败</span>"
                    },
                ));
                let call = data["call_id"].as_str().unwrap_or_default();
                let text = full
                    .get(call)
                    .or_else(|| results.get(call))
                    .cloned()
                    .unwrap_or_else(|| {
                        data["output_detail"]
                            .as_str()
                            .or(data["output_preview"].as_str())
                            .unwrap_or_default()
                            .into()
                    });
                body.push_str(&format!(
                    "<pre class=\"preview\">{}</pre>",
                    escape(util::prefix(&text, 500))
                ));
                body.push_str(&details("完整工具输出", &text));
                if !data["error"].is_null() {
                    body.push_str(&kv(
                        "错误",
                        &format!("<code class=\"err\">{}</code>", display(&data["error"])),
                    ));
                }
            }
            "goal_check" => {
                if let Some(items) = data["items"].as_array() {
                    for item in items {
                        let mark = if item["passed"] == true { "✓" } else { "✗" };
                        let id = item
                            .get("item_id")
                            .or_else(|| item.get("name"))
                            .unwrap_or(&Value::Null);
                        goal_rows.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{mark}</td><td>{}</td><td class=\"ev-time\">{}</td></tr>",display(id),display(&item["kind"]),display(&item["exit_code"]),escape(&event.timestamp)));
                        body.push_str(&kv(
                            &display(id),
                            &format!("{mark} {}", display(&item["detail"])),
                        ));
                    }
                }
            }
            "session_end" => {
                body.push_str(&kv("退出原因", &display(&data["exit_reason"])));
                body.push_str(&kv("轮数", &display(&data["rounds"])));
                if data["error"].is_string() {
                    body.push_str(&kv("错误", &display(&data["error"])));
                }
            }
            "context_compacted" => body.push_str(&kv(
                "Token",
                &format!(
                    "{} → {}",
                    display(&data["tokens_before"]),
                    display(&data["tokens_after"])
                ),
            )),
            "approval_decision" => body.push_str(&kv(
                "审批",
                if data["granted"] == true {
                    "✓ 允许"
                } else {
                    "✗ 拒绝"
                },
            )),
            _ => body.push_str(&details(
                "事件数据",
                &serde_json::to_string_pretty(data).unwrap_or_default(),
            )),
        }
        timeline.push_str(&format!("<div class=\"event {state}\"><div class=\"ev-head\"><span class=\"badge b-{category}\">{}</span><code class=\"ev-type\">{}</code><span class=\"ev-time\">#{} · {}</span></div><div class=\"ev-body\">{body}</div></div>",escape(label),escape(&event.r#type),event.seq,escape(&event.timestamp)));
    }
    let goals = if goal_rows.is_empty() {
        "<p class=\"muted\">（无 goal_check 事件）</p>".into()
    } else {
        format!(
            "<table class=\"goal-table\"><thead><tr><th>item</th><th>kind</th><th>通过</th><th>退出码</th><th>时间</th></tr></thead><tbody>{goal_rows}</tbody></table>"
        )
    };
    let footer = format!(
        "<footer class=\"panel\"><h2>用量</h2><p>输入 Token <strong>{input}</strong> · 输出 Token <strong>{output}</strong> · 总计 <strong>{}</strong> · 缓存读取 <strong>{}</strong> · 缓存写入 <strong>{}</strong> · 轮数 <strong>{}</strong></p><p class=\"muted\">单文件离线报告：无外部 CSS / JS / 字体依赖，无需 JavaScript 即可查看全部内容（折叠区使用原生 details 元素）。</p></footer>",
        input + output,
        display(&summary["cache_read_tokens"]),
        display(&summary["cache_write_tokens"]),
        display(&summary["rounds"])
    );
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Mini Claude Code 执行报告 — {id}</title><style>{}</style></head><body><div class=\"wrap\">{header}<section class=\"panel\"><h2>目标验收（Goal Checks）</h2>{goals}</section><section class=\"panel\"><h2>时间线</h2><div class=\"timeline\">{timeline}</div></section>{footer}</div></body></html>\n",
        include_str!("assets/report.css")
    )
}
