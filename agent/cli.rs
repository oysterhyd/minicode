use crate::{
    bridge,
    configuration::Configuration,
    extensions::Catalog,
    goals,
    models::Budget,
    runtime::{Hooks, Ledger, Runtime, Settings, Snapshot},
    store::Store,
    tools::Sink,
    util,
};
use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU64},
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "minicode",
    version,
    about = "MiniCode 本地编码助手",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// NDJSON protocol v2 host for Desktop and Ink TUI.
    Bridge,
    /// 执行一个任务并保存会话。
    Run {
        task: String,
        #[command(flatten)]
        options: RunOptions,
    },
    /// 交互式持续会话。
    Chat {
        #[command(flatten)]
        options: RunOptions,
    },
    /// 恢复持久会话并继续交互。
    Resume {
        session_id: String,
        #[command(flatten)]
        options: RunOptions,
    },
    /// TypeScript + Ink 终端界面。
    Tui {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Report {
        session_id: String,
        #[arg(long)]
        full: bool,
        #[arg(long, default_value = "text")]
        format: String,
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    Plugins {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    Eval {
        #[command(flatten)]
        options: crate::eval::Options,
    },
}
#[derive(Args, Clone, Default)]
struct RunOptions {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    provider: String,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    script: Option<PathBuf>,
    #[arg(long, default_value_t = 0)]
    max_rounds: u64,
    #[arg(long, default_value_t = 0)]
    max_tokens: i64,
    #[arg(long, default_value_t = 0.0)]
    max_seconds: f64,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    acceptance: Option<PathBuf>,
}
#[derive(Subcommand)]
enum SessionCommand {
    List,
}
#[derive(Subcommand)]
enum PluginCommand {
    List {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    Lock {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
}
#[derive(Subcommand)]
enum MemoryCommand {
    Add {
        fact: String,
        #[arg(long)]
        source: String,
        #[arg(long, default_value = ".")]
        scope: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    List {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    Update {
        memory_id: String,
        fact: String,
        #[arg(long)]
        source: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    Delete {
        memory_id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
}
#[derive(Subcommand)]
enum WorkflowCommand {
    Review {
        #[arg(long)]
        check: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "deepseek/deepseek-v4.1-flash")]
        model: String,
        #[arg(long)]
        retry_unknown: bool,
    },
}

pub async fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli
        .db
        .or_else(|| std::env::var_os("MINICODE_DB_PATH").map(PathBuf::from))
        .unwrap_or(util::data_dir().join("sessions.db"));
    match cli.command {
        Commands::Bridge => bridge::serve(Store::open(path)?).await,
        Commands::Tui { args } => launch_tui(&args),
        Commands::Run { task, options } => {
            let store = Store::open(path)?;
            let (mut runtime, hooks) = prepare(&store, None, &options).await?;
            banner(&runtime);
            let result = run(&mut runtime, Some(task), hooks).await?;
            println!(
                "\n会话 {} · {} · 轮数 {} · Token {}",
                result.session_id,
                result.exit_reason,
                result.rounds,
                result.total_usage.total()
            );
            if result.exit_reason != "completed" {
                std::process::exit(if result.exit_reason == "cancelled" {
                    130
                } else {
                    2
                })
            }
            Ok(())
        }
        Commands::Chat { options } => {
            let store = Store::open(path)?;
            let (runtime, hooks) = prepare(&store, None, &options).await?;
            chat(runtime, hooks).await
        }
        Commands::Resume {
            session_id,
            options,
        } => {
            let store = Store::open(path)?;
            let id = resolve(&store, &session_id)?;
            let (runtime, hooks) = prepare(&store, Some(&id), &options).await?;
            println!(
                "恢复会话 {} · 轮数 {} · Token {}",
                &id[..8],
                runtime.rounds,
                runtime.usage.total()
            );
            chat(runtime, hooks).await
        }
        Commands::Sessions { .. } => {
            for s in Store::open(path)?.sessions()? {
                println!(
                    "{} · {} · {} · {} · {} rounds",
                    s["session_id"].as_str().unwrap(),
                    s["created_at"].as_str().unwrap(),
                    s["workspace"].as_str().unwrap(),
                    s["status"].as_str().unwrap(),
                    s["rounds"]
                );
            }
            Ok(())
        }
        Commands::Report {
            session_id,
            full,
            format,
            output,
        } => report(
            &Store::open(path)?,
            &session_id,
            full,
            &format,
            output.as_deref(),
        ),
        Commands::Plugins { command } => {
            let (workspace, lock) = match command {
                PluginCommand::List { workspace } => (workspace, false),
                PluginCommand::Lock { workspace } => (workspace, true),
            };
            let catalog = Catalog::scan(&workspace, Configuration::default(), !lock)?;
            if lock {
                println!("{}", catalog.write_lock()?.display());
            } else {
                for p in catalog.plugins {
                    println!(
                        "{} {} {} sha256:{}",
                        p["name"].as_str().unwrap(),
                        p["version"].as_str().unwrap(),
                        if p["enabled"] == true {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        p["digest"].as_str().unwrap()
                    );
                }
            }
            Ok(())
        }
        Commands::Memory { command } => memory(&Store::open(path)?, command),
        Commands::Workflow {
            command:
                WorkflowCommand::Review {
                    check,
                    output,
                    workspace,
                    model,
                    retry_unknown,
                },
        } => review(&workspace, &output, &check, &model, retry_unknown).await,
        Commands::Eval { options } => crate::eval::run(options).await,
    }
}
fn resolve(store: &Store, prefix: &str) -> Result<String> {
    let matches = store
        .sessions()?
        .into_iter()
        .filter(|s| {
            s["session_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!("会话 ID 不唯一或不存在")
    };
    Ok(matches[0]["session_id"].as_str().unwrap().into())
}
async fn prepare(
    store: &Store,
    session: Option<&str>,
    options: &RunOptions,
) -> Result<(Runtime, Hooks)> {
    let configuration = Configuration::default();
    let mut settings = Settings::new(&configuration)?;
    let summary = session.map(|id| store.require(id)).transpose()?;
    let workspace = options
        .workspace
        .clone()
        .or_else(|| {
            summary
                .as_ref()
                .and_then(|s| s["workspace"].as_str().map(PathBuf::from))
        })
        .unwrap_or(PathBuf::from("."));
    let provider = options.provider.as_str();
    if !["auto", "fake", "anthropic", "commandcode"].contains(&provider) {
        bail!("未知 provider：{provider}")
    }
    settings.model = if provider == "fake" || options.script.is_some() {
        "fake".into()
    } else {
        options
            .model
            .clone()
            .or_else(|| {
                summary
                    .as_ref()
                    .and_then(|s| s["model"].as_str().map(str::to_string))
            })
            .unwrap_or_else(|| {
                if !settings.model.is_empty() {
                    settings.model.clone()
                } else if provider == "anthropic" {
                    "claude-sonnet-4-5".into()
                } else {
                    "deepseek/deepseek-v4.1-flash".into()
                }
            })
    };
    settings.mode = if options.yes { "bypass" } else { "default" }.into();
    settings.budget = Budget {
        max_rounds: options.max_rounds,
        max_total_tokens: options.max_tokens,
        max_seconds: options.max_seconds,
    };
    settings.acceptance = options.acceptance.clone();
    if let Some(id) = session
        && settings.budget.max_total_tokens == 0
        && let Some(saved) = store
            .events(id, -1)?
            .iter()
            .rev()
            .find(|e| ["budget_changed", "session_start"].contains(&e.r#type.as_str()))
    {
        settings.budget.max_total_tokens = saved.data["budget"]["max_total_tokens"]
            .as_i64()
            .unwrap_or(0);
    }
    let mut runtime = Runtime::new(
        &workspace,
        &settings.model,
        session,
        store.clone(),
        configuration,
        &settings,
    )
    .await?;
    if let Some(script) = &options.script {
        let value: Value = serde_json::from_slice(&fs::read(script)?)?;
        runtime.provider.set_script(value)?;
    }
    if provider != "auto" && runtime.provider.name != provider {
        bail!("模型不属于 provider {provider}")
    }
    let sink: Sink = Arc::new(|event| {
        let event_type = event["event"].as_str().unwrap_or_default();
        if event_type == "text_delta" {
            print!("{}", event["text"].as_str().unwrap_or_default());
            let _ = io::stdout().flush();
        } else if event_type == "agent_event" {
            let item = &event["item"];
            match item["type"].as_str() {
                Some("tool_call_start") => println!(
                    "\n▶ {} {}",
                    item["data"]["name"].as_str().unwrap_or_default(),
                    item["data"]["arguments"]
                ),
                Some("tool_call_result") => println!(
                    "{} {}",
                    if item["data"]["success"] == true {
                        "✓"
                    } else {
                        "✗"
                    },
                    item["data"]["output_detail"].as_str().unwrap_or_default()
                ),
                _ => {}
            }
        }
    });
    let hooks = Hooks {
        settings: Arc::new(Mutex::new(settings)),
        snapshot: Arc::new(Mutex::new(Snapshot::default())),
        approvals: Arc::new(Mutex::new(HashMap::new())),
        approval_sequence: Arc::new(AtomicU64::new(0)),
        sink,
        cancel: CancellationToken::new(),
        shared: Arc::new(Mutex::new(Ledger::default())),
    };
    Ok((runtime, hooks))
}
fn banner(runtime: &Runtime) {
    println!(
        "{}\nminicode v{} · Rust · {} · {}/{} · {}\n",
        include_str!("assets/banner.txt"),
        crate::VERSION,
        std::env::consts::OS,
        runtime.provider.name,
        runtime.provider.configuration_id,
        runtime.workspace.display()
    );
}
async fn run(
    runtime: &mut Runtime,
    text: Option<String>,
    mut hooks: Hooks,
) -> Result<crate::models::RunResult> {
    hooks.cancel = CancellationToken::new();
    let approvals = hooks.approvals.clone();
    let sink = hooks.sink.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Value>();
    hooks.sink = Arc::new(move |event| {
        if event["event"] == "approval" {
            let _ = sender.send(event);
        } else {
            sink(event);
        }
    });
    let cancel = hooks.cancel.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {_ = cancel.cancelled()=>break,Some(event)=receiver.recv()=>{
                println!("\n审批：{}\n允许执行？[y/N]",event["request"]["summary"].as_str().unwrap_or_default());let line=tokio::task::spawn_blocking(||{let mut s=String::new();io::stdin().read_line(&mut s).map(|_|s)}).await??;
                if let Some((_,tx))=approvals.lock().unwrap().remove(event["approvalId"].as_str().unwrap_or_default()){let _=tx.send(matches!(line.trim().to_lowercase().as_str(),"y"|"yes"));}
            },else=>break}
        }
        Ok::<(), anyhow::Error>(())
    });
    let cancel = hooks.cancel.clone();
    let ctrl = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel.cancel();
        }
    });
    let result = runtime.run(text, vec![], hooks).await;
    task.abort();
    ctrl.abort();
    result
}
async fn chat(mut runtime: Runtime, hooks: Hooks) -> Result<()> {
    banner(&runtime);
    println!("/help 查看命令 · Ctrl+C 取消 · Ctrl+D 退出");
    loop {
        print!("\n❯ ");
        io::stdout().flush()?;
        let line = tokio::task::spawn_blocking(|| {
            let mut s = String::new();
            io::stdin().read_line(&mut s).map(|n| (n, s))
        })
        .await??;
        if line.0 == 0 {
            break;
        }
        let text = line.1.trim().to_string();
        if ["exit", "quit", "/exit", "/quit"].contains(&text.as_str()) {
            break;
        }
        if text.is_empty() {
            continue;
        }
        if text == "/continue" {
            if !runtime.pending {
                println!("当前没有暂停的任务");
                continue;
            }
            let result = run(&mut runtime, None, hooks.clone()).await?;
            println!("\n{}", result.exit_reason);
            continue;
        }
        if text == "/compact" {
            runtime.compact(&hooks).await?;
            continue;
        }
        if text == "/clear" {
            print!("\x1b[2J\x1b[H");
            continue;
        }
        if text == "/help" || text == "/?" {
            for c in serde_json::from_str::<Vec<Value>>(include_str!("assets/commands.json"))? {
                println!(
                    "{}  {}",
                    c["usage"].as_str().unwrap(),
                    c["summary"].as_str().unwrap()
                );
            }
            continue;
        }
        if text == "/sessions" {
            for s in runtime.store.sessions()? {
                println!("{} {}", s["session_id"], s["title"]);
            }
            continue;
        }
        if let Some(arg) = text.strip_prefix("/model ") {
            let config = runtime.catalog.lock().await.configuration.clone();
            let provider = crate::provider::Provider::new(
                arg,
                &hooks.settings.lock().unwrap().effort,
                &config,
            )?;
            runtime.provider = provider;
            hooks.settings.lock().unwrap().model = arg.into();
            continue;
        }
        if let Some(arg) = text.strip_prefix("/effort ") {
            if !["off", "low", "medium", "high", "xhigh", "max"].contains(&arg) {
                bail!("无效 reasoning effort")
            };
            runtime.provider.effort = arg.into();
            hooks.settings.lock().unwrap().effort = arg.into();
            continue;
        }
        if let Some(arg) = text.strip_prefix("/permissions ") {
            if !["default", "accept_edits", "bypass"].contains(&arg) {
                bail!("无效权限模式")
            };
            hooks.settings.lock().unwrap().mode = arg.into();
            continue;
        }
        if let Some(arg) = text.strip_prefix("/skill ") {
            let mut catalog = runtime.catalog.lock().await;
            if let Some(name) = arg.strip_prefix("off ") {
                catalog.unload(name)?;
                runtime.event(&hooks, "skill_deactivated", json!({"name":name}))?;
            } else {
                let s = catalog.load(arg)?;
                runtime.event(
                    &hooks,
                    "skill_activated",
                    json!({"name":arg,"sha256":s.digest,"path":s.path}),
                )?;
            }
            continue;
        }
        if text == "/new" {
            let options = RunOptions {
                workspace: Some(runtime.workspace.clone()),
                model: Some(runtime.provider.configuration_id.clone()),
                provider: "auto".into(),
                ..RunOptions::default()
            };
            let (new, _) = prepare(&runtime.store, None, &options).await?;
            runtime = new;
            continue;
        }
        if let Some(id) = text.strip_prefix("/resume ") {
            let id = resolve(&runtime.store, id)?;
            let settings = hooks.settings.lock().unwrap().clone();
            let configuration = runtime.catalog.lock().await.configuration.clone();
            let s = runtime.store.require(&id)?;
            runtime = Runtime::new(
                Path::new(util::string(&s, "workspace")?),
                util::string(&s, "model")?,
                Some(&id),
                runtime.store.clone(),
                configuration,
                &settings,
            )
            .await?;
            continue;
        }
        if text.starts_with('/') {
            println!("未知命令：{text}");
            continue;
        }
        let result = run(&mut runtime, Some(text), hooks.clone()).await?;
        println!(
            "\n{} · Token {}",
            result.exit_reason,
            result.total_usage.total()
        );
    }
    Ok(())
}
fn runtime_dir() -> Option<PathBuf> {
    if let Some(resources) = std::env::var_os("MINICODE_RESOURCES") {
        return Some(PathBuf::from(resources).join("runtime"));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.join("tui/dist/index.js").is_file() {
        Some(dir.into())
    } else {
        None
    }
}
fn launch_tui(args: &[String]) -> Result<()> {
    let runtime = runtime_dir();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let entry = runtime
        .as_ref()
        .map(|r| r.join("tui/dist/index.js"))
        .unwrap_or(root.join("tui/dist/index.js"));
    if !entry.is_file() {
        bail!("TUI 尚未构建，请运行 npm ci --prefix tui && npm run build --prefix tui")
    };
    let node = runtime.as_ref().map(|r| {
        r.join(if cfg!(windows) {
            "node/node.exe"
        } else {
            "node/node"
        })
    });
    let mut command = std::process::Command::new(
        node.filter(|p| p.is_file())
            .unwrap_or(PathBuf::from("node")),
    );
    command
        .arg(entry)
        .args(args)
        .env("MINICODE_AGENT", std::env::current_exe()?);
    let status = command.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1))
    }
    Ok(())
}
fn memory(store: &Store, command: MemoryCommand) -> Result<()> {
    match command {
        MemoryCommand::List { workspace } => {
            let workspace = util::canonical(&workspace)?;
            for m in store.memories(&workspace.to_string_lossy())? {
                println!(
                    "{} [{}] {} (source: {}; updated: {})",
                    m["id"].as_str().unwrap(),
                    m["scope"].as_str().unwrap(),
                    m["fact"].as_str().unwrap(),
                    m["source"].as_str().unwrap(),
                    m["updated_at"].as_str().unwrap()
                );
            }
        }
        MemoryCommand::Add {
            fact,
            source,
            scope,
            workspace,
        } => {
            if fact.trim().is_empty() || source.trim().is_empty() {
                bail!("fact and source must be nonempty")
            };
            let workspace = util::canonical(&workspace)?;
            let path = util::in_workspace(&workspace, &scope)?;
            let scope = path
                .strip_prefix(&workspace)?
                .to_string_lossy()
                .replace('\\', "/");
            let scope = if scope.is_empty() { ".".into() } else { scope };
            let id = util::id();
            store.with(|c| {
                c.execute(
                    "INSERT INTO project_memory VALUES(?,?,?,?,?,?)",
                    rusqlite::params![
                        id,
                        workspace.to_string_lossy(),
                        scope,
                        fact.trim(),
                        source.trim(),
                        util::now()
                    ],
                )?;
                Ok(())
            })?;
            println!("已保存 {id}（范围 {scope}）");
        }
        MemoryCommand::Update {
            memory_id,
            fact,
            source,
            workspace,
        } => {
            if fact.trim().is_empty() || source.trim().is_empty() {
                bail!("fact and source must be nonempty")
            };
            let workspace = util::canonical(&workspace)?;
            let count=store.with(|c|Ok(c.execute("UPDATE project_memory SET fact=?,source=?,updated_at=? WHERE workspace=? AND id=?",rusqlite::params![fact.trim(),source.trim(),util::now(),workspace.to_string_lossy(),memory_id])?))?;
            if count != 1 {
                bail!("unknown memory id: {memory_id}")
            };
            println!("已更新 {memory_id}");
        }
        MemoryCommand::Delete {
            memory_id,
            workspace,
        } => {
            let workspace = util::canonical(&workspace)?;
            let count = store.with(|c| {
                Ok(c.execute(
                    "DELETE FROM project_memory WHERE workspace=? AND id=?",
                    rusqlite::params![workspace.to_string_lossy(), memory_id],
                )?)
            })?;
            if count != 1 {
                bail!("unknown memory id: {memory_id}")
            };
            println!("已删除 {memory_id}");
        }
    }
    Ok(())
}
fn report(
    store: &Store,
    prefix: &str,
    full: bool,
    format: &str,
    output: Option<&Path>,
) -> Result<()> {
    let id = resolve(store, prefix)?;
    let summary = store.require(&id)?;
    let messages = store.messages(&id)?;
    let events = store.events(&id, -1)?;
    let mut sections = vec![format!(
        "会话 {id}\n工作区: {}\n模型: {}/{}\n状态: {}",
        summary["workspace"].as_str().unwrap(),
        summary["provider"].as_str().unwrap(),
        summary["model"].as_str().unwrap(),
        summary["status"].as_str().unwrap()
    )];
    for message in messages {
        let text = message.text();
        if !text.is_empty() {
            sections.push(format!("{}\n{text}", message.role));
        }
    }
    for event in events {
        if event.r#type == "tool_call_result" {
            let mut text = event.data["output_detail"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if full {
                if let Some(artifact) = event.data["artifact_id"].as_str()
                    && let Ok(path) = store.artifact_path(&id, artifact)
                {
                    text = fs::read_to_string(path)?;
                }
            } else {
                text = text.chars().take(500).collect();
            }
            sections.push(format!(
                "{} · success={}\n{}\n{}",
                event.data["name"].as_str().unwrap_or_default(),
                event.data["success"],
                text,
                event.data["error"].as_str().unwrap_or_default()
            ));
        }
    }
    if format == "text" {
        println!("{}", sections.join("\n\n"));
    } else if format == "html" {
        let path = output
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("minicode-report-{}.html", &id[..8])));
        let events = store.events(&id, -1)?;
        let messages = store.messages(&id)?;
        let mut full_outputs = HashMap::new();
        for event in &events {
            if let (Some(call), Some(artifact)) = (
                event.data["call_id"].as_str(),
                event.data["artifact_id"].as_str(),
            ) && let Ok(file) = store.artifact_path(&id, artifact)
            {
                full_outputs.insert(call.to_string(), fs::read_to_string(file)?);
            }
        }
        let html = crate::report::render(&summary, &events, &messages, &full_outputs);
        util::atomic_write(&path, html.as_bytes())?;
        println!("HTML 报告已生成: {}", path.display());
    } else {
        bail!("未知格式: {format}（可选 text / html）")
    }
    Ok(())
}
async fn review(
    workspace: &Path,
    output: &Path,
    check: &str,
    model: &str,
    retry: bool,
) -> Result<()> {
    let workspace = util::canonical(workspace)?;
    let output = util::canonical(output)?;
    if output.starts_with(&workspace) {
        bail!("review output must be outside the workspace being reviewed")
    }
    let fingerprint = goals::fingerprint(&workspace)?;
    let path = output.join("journal.json");
    let mut journal = if path.is_file() {
        let v: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if v["workspace"] != workspace.to_string_lossy().as_ref()
            || v["check_command"] != check
            || v["model"] != model
            || v["fingerprint"] != fingerprint
        {
            bail!("review journal belongs to a different workspace, check, model or fingerprint")
        };
        v
    } else {
        if output.is_dir() && fs::read_dir(&output)?.next().is_some() {
            bail!("new review output directory must be empty")
        };
        json!({"workspace":workspace,"check_command":check,"model":model,"fingerprint":fingerprint,"created_at":util::now(),"snapshot":"pending","check":"pending","review":"pending","check_exit_code":null,"session_id":null})
    };
    let save = |v: &Value| util::atomic_write(&path, &serde_json::to_vec_pretty(v)?);
    save(&journal)?;
    if journal["snapshot"] != "done" {
        let mut text = String::new();
        for args in [
            vec!["status", "--short"],
            vec!["diff", "--no-ext-diff", "--binary"],
        ] {
            let result = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(&workspace)
                .output()
                .await?;
            if !result.status.success() {
                bail!("git snapshot failed")
            };
            text.push_str(&String::from_utf8_lossy(&result.stdout));
        }
        util::atomic_write(&output.join("diff.patch"), text.as_bytes())?;
        journal["snapshot"] = json!("done");
        save(&journal)?;
    }
    if journal["check"] == "running" {
        journal["check"] = json!("unknown");
        save(&journal)?;
    }
    if journal["check"] == "unknown" && !retry {
        println!("检查命令在中断时状态未知；核实其效果后用 --retry-unknown 续跑。");
        std::process::exit(2)
    }
    let store = Store::open(output.join("review.sqlite3"))?;
    let options = RunOptions {
        workspace: Some(workspace.clone()),
        model: Some(model.into()),
        provider: "auto".into(),
        max_rounds: 8,
        max_tokens: 80_000,
        max_seconds: 300.0,
        ..RunOptions::default()
    };
    let session = journal["session_id"].as_str().map(str::to_string);
    let (mut runtime, hooks) = prepare(&store, session.as_deref(), &options).await?;
    if journal["check"] != "done" {
        journal["check"] = json!("running");
        save(&journal)?;
        let result =
            crate::tools::run_command(check, &workspace, 120.0, &runtime.context(&hooks, None))
                .await?;
        util::atomic_write(&output.join("check.log"), result.output.as_bytes())?;
        journal["check_exit_code"] = json!(result.exit_code);
        journal["check"] = json!("done");
        save(&journal)?;
        if goals::fingerprint(&workspace)? != fingerprint {
            bail!("check command changed workspace source; review stopped")
        }
    }
    if journal["review"] == "done" {
        return Ok(());
    }
    runtime.allowed = Some(
        ["read", "ls", "grep", "read_artifact"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    journal["review"] = json!("running");
    journal["session_id"] = json!(runtime.session);
    save(&journal)?;
    let prompt = if runtime.pending {
        None
    } else {
        Some(format!(
            "只读审查当前代码改动。列出有证据的缺陷，给出文件与行号；没有发现则说明检查范围。不得修改文件。\n{}\n检查退出码：{}\n检查输出：{}",
            util::suffix(&fs::read_to_string(output.join("diff.patch"))?, 20_000),
            journal["check_exit_code"],
            util::suffix(&fs::read_to_string(output.join("check.log"))?, 8000)
        ))
    };
    let result = run(&mut runtime, prompt, hooks).await?;
    journal["review"] = json!(if result.exit_reason == "completed" {
        "done"
    } else {
        "paused"
    });
    journal["review_exit_reason"] = json!(result.exit_reason);
    util::atomic_write(
        &output.join("review.md"),
        runtime
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.text())
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )?;
    save(&journal)?;
    println!(
        "review={} check_exit={} output={}",
        journal["review"],
        journal["check_exit_code"],
        output.display()
    );
    Ok(())
}
