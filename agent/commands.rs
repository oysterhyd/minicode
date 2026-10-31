use crate::{configuration::Configuration, extensions::Catalog, store::Store, util};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub fn skill_argument(arg: &str) -> Result<Option<(bool, &str)>> {
    let words = arg.split_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        [] => Ok(None),
        [name] if !name.eq_ignore_ascii_case("off") && !name.eq_ignore_ascii_case("on") => {
            Ok(Some((true, name)))
        }
        [command, name]
            if command.eq_ignore_ascii_case("off") || command.eq_ignore_ascii_case("on") =>
        {
            Ok(Some((!command.eq_ignore_ascii_case("off"), name)))
        }
        _ => bail!("用法：/skill [名称 | on 名称 | off 名称]"),
    }
}

pub fn information(
    verb: &str,
    arg: &str,
    workspace: &Path,
    session: Option<&str>,
    state: &Value,
    store: &Store,
) -> Result<Option<String>> {
    if ![
        "/cost", "/context", "/doctor", "/init", "/agents", "/mcp", "/memory", "/export", "/undo",
        "/rewind", "/fork",
    ]
    .contains(&verb)
    {
        return Ok(None);
    }
    if verb != "/export" && !arg.is_empty() {
        bail!("{verb} 不接受参数");
    }
    let text = match verb {
        "/undo" | "/rewind" | "/fork" => bail!("{verb} 依赖后续 G3 会话检查点能力；尚未实现"),
        "/context" => format!(
            "上下文估算：{} / {} tokens\n{}",
            state["contextTokens"], state["contextWindow"], state["contextBreakdown"]
        ),
        "/cost" => {
            let summary = session
                .map(|id| store.require(id))
                .transpose()?
                .unwrap_or(json!({}));
            format!(
                "会话用量：input={} output={} cache_read={} cache_write={}\n费用：不可用（没有可核实的模型定价）",
                summary["input_tokens"].as_u64().unwrap_or(0),
                summary["output_tokens"].as_u64().unwrap_or(0),
                summary["cache_read_tokens"].as_u64().unwrap_or(0),
                summary["cache_write_tokens"].as_u64().unwrap_or(0)
            )
        }
        "/doctor" => format!(
            "MiniCode {}\nOS: {}\nworkspace: {}\nmodel: {}\nGit: {}",
            crate::VERSION,
            std::env::consts::OS,
            workspace.display(),
            state["model"].as_str().unwrap_or("未配置"),
            if std::process::Command::new("git")
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                "available"
            } else {
                "unavailable"
            }
        ),
        "/init" => {
            let file = workspace.join("AGENTS.md");
            if file.exists() {
                bail!("AGENTS.md 已存在；请编辑现有项目约定");
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)?;
            output.write_all(b"# Project instructions\n\nFollow the existing architecture and conventions.\nRead files before editing; preserve user changes.\nRun focused tests after changes and report verification.\n")?;
            "已创建 AGENTS.md；请补充项目的构建与测试命令。".into()
        }
        "/memory" => serde_json::to_string_pretty(&store.memories(&workspace.to_string_lossy())?)?,
        "/agents" | "/mcp" => {
            let catalog = Catalog::scan(workspace, Configuration::default(), false)?;
            if verb == "/agents" {
                serde_json::to_string_pretty(
                    &json!({"configured":Configuration::default().agents()?.iter().map(|agent|agent["name"].clone()).collect::<Vec<_>>(),"local":catalog.agents.keys().collect::<Vec<_>>()}),
                )?
            } else {
                format!(
                    "已配置 MCP（连接状态请使用客户端测试）：{}",
                    catalog
                        .servers
                        .iter()
                        .map(|server| server.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        "/export" => {
            let id = session.ok_or_else(|| anyhow::anyhow!("没有可导出的会话"))?;
            let destination = if arg.is_empty() {
                format!("minicode-{id}.json")
            } else {
                arg.trim_matches('"').to_string()
            };
            let file = util::in_workspace(workspace, &destination)?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)?;
            output.write_all(&serde_json::to_vec_pretty(
                &json!({"session":store.require(id)?,"messages":store.messages(id)?}),
            )?)?;
            format!("已导出 {}", file.display())
        }
        _ => unreachable!(),
    };
    Ok(Some(text))
}
