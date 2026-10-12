use crate::{
    configuration::Configuration,
    goals,
    models::Budget,
    runtime::{Hooks, Ledger, Runtime, Settings, Snapshot},
    store::Store,
    tools, util,
};
use anyhow::{Context, Result, bail};
use clap::Args;
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Args, Clone)]
pub struct Options {
    #[arg(long, default_value = "evals/tasks")]
    tasks_dir: PathBuf,
    #[arg(long)]
    task: Vec<String>,
    #[arg(long, default_value = "b0,b2")]
    baselines: String,
    #[arg(long, short, default_value = "reports/eval")]
    output: PathBuf,
    /// Use a configured real model instead of deterministic offline scripts.
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = 1)]
    repeats: usize,
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
    #[arg(long, default_value = "alternating", value_parser = ["alternating", "grouped"])]
    schedule: String,
}

fn snapshot(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for entry in util::walk(root).filter(|e| !e.file_type().is_dir()) {
        if util::is_link(entry.path()) {
            bail!("evaluation fixture contains a link")
        }
        files.insert(
            entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/"),
            goals::hash_file(entry.path())?,
        );
    }
    Ok(files)
}
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(source).into_iter() {
        let entry = entry?;
        if util::is_link(entry.path()) {
            bail!("evaluation source contains a link")
        }
        let target = destination.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            fs::create_dir_all(target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn allowed_failures(
    spec: &Value,
    before: &BTreeMap<String, String>,
    workspace: &Path,
) -> Result<Vec<String>> {
    let after = snapshot(workspace)?;
    let mut builder = globset::GlobSetBuilder::new();
    for entry in spec["allowed_paths"]
        .as_array()
        .context("task requires allowed_paths")?
    {
        builder.add(globset::Glob::new(
            entry.as_str().context("invalid allowed path")?,
        )?);
    }
    let allowed = builder.build()?;
    let changed = before
        .keys()
        .chain(after.keys())
        .filter(|p| before.get(*p) != after.get(*p))
        .collect::<std::collections::BTreeSet<_>>();
    Ok(changed
        .into_iter()
        .filter(|p| !allowed.is_match(p))
        .map(|p| format!("changed disallowed file: {p}"))
        .collect())
}
async fn grade(
    spec: &Value,
    before: &BTreeMap<String, String>,
    runtime: &Runtime,
    hooks: &Hooks,
) -> Result<Vec<String>> {
    // Grading owns an independent cancellation/deadline, after the agent stops.
    let mut ctx = runtime.context(hooks, None);
    ctx.cancel = CancellationToken::new();
    ctx.deadline = None;
    let mut failures = allowed_failures(spec, before, &runtime.workspace)?;
    for item in spec["items"]
        .as_array()
        .context("task requires acceptance items")?
    {
        match item["type"].as_str() {
            Some("command") => {
                let result = tools::run_command(
                    util::string(item, "command")?,
                    &runtime.workspace,
                    120.0,
                    &ctx,
                )
                .await?;
                if !result.success {
                    failures.push(format!(
                        "{}: {}",
                        item["id"],
                        util::suffix(&result.output, 3000)
                    ));
                }
            }
            Some("protected") => {
                let raw = util::string(item, "path")?;
                let path = util::in_workspace(&runtime.workspace, raw)?;
                let current = if path.is_file() {
                    Some(goals::hash_file(&path)?)
                } else {
                    None
                };
                if current.as_ref() != before.get(raw) {
                    failures.push(format!("protected file changed: {raw}"));
                }
            }
            Some("artifact") => {
                if !util::in_workspace(&runtime.workspace, util::string(item, "path")?)?.is_file() {
                    failures.push(format!("missing artifact: {}", item["path"]));
                }
            }
            _ => bail!("unknown acceptance item type"),
        }
    }
    // Check all protected_paths, including those not repeated in items.
    for raw in spec["protected_paths"]
        .as_array()
        .context("task requires protected_paths")?
    {
        let raw = raw.as_str().context("invalid protected path")?;
        let path = util::in_workspace(&runtime.workspace, raw)?;
        let current = if path.is_file() {
            Some(goals::hash_file(&path)?)
        } else {
            None
        };
        if current.as_ref() != before.get(raw) {
            failures.push(format!("protected file changed: {raw}"));
        }
    }
    Ok(failures)
}
async fn one(
    options: &Options,
    task: &Path,
    baseline: &str,
    repeat: usize,
    cancel: CancellationToken,
) -> Result<Value> {
    let id = task
        .file_name()
        .context("invalid task directory")?
        .to_string_lossy();
    let spec: Value = serde_yaml::from_str(&fs::read_to_string(task.join("task.yaml"))?)?;
    let fixture = util::hash(serde_json::to_vec(&json!({"definition":spec,"files":snapshot(&task.join("repo"))?,"script":goals::hash_file(&task.join("script.json"))?}))?.as_slice());
    let model = options.model.as_deref().unwrap_or("fake");
    let run_dir = options
        .output
        .join("runs")
        .join(id.as_ref())
        .join(baseline)
        .join(repeat.to_string());
    let record_path = run_dir.join("record.json");
    if record_path.is_file() {
        let record: Value = serde_json::from_slice(&fs::read(&record_path)?)?;
        if record["model"] != model || record["fixture_sha256"] != fixture {
            bail!(
                "existing evaluation record has different model or fixture: {}",
                record_path.display()
            )
        }
        return Ok(record);
    }
    fs::create_dir_all(&run_dir)?;
    // Existing interrupted work remains available for inspection; each attempt
    // starts from a clean copy and never implicitly replays side effects.
    let workspace = run_dir.join(format!("workspace-{}", util::id()));
    copy_tree(&task.join("repo"), &workspace)?;
    let before = snapshot(&workspace)?;
    let started = Instant::now();
    let settings = Settings {
        model: model.into(),
        mode: "bypass".into(),
        budget: Budget {
            max_rounds: spec["budget"]["max_rounds"].as_u64().unwrap_or(64),
            max_total_tokens: spec["budget"]["max_tokens"].as_i64().unwrap_or(120_000),
            max_seconds: spec["budget"]["max_seconds"].as_f64().unwrap_or(180.0),
        },
        ..Settings::new(&Configuration::default())?
    };
    let store = Store::open(run_dir.join(format!("session-{}.db", util::id())))?;
    let mut runtime = Runtime::new(
        &workspace,
        model,
        None,
        store,
        Configuration::default(),
        &settings,
    )
    .await?;
    if model == "fake" {
        runtime
            .provider
            .set_script(serde_json::from_slice(&fs::read(
                task.join("script.json"),
            )?)?)?;
    }
    runtime.compaction = baseline != "b0";
    let hooks = Hooks {
        settings: Arc::new(Mutex::new(settings)),
        snapshot: Arc::new(Mutex::new(Snapshot::default())),
        approvals: Arc::new(Mutex::new(HashMap::new())),
        approval_sequence: Arc::new(AtomicU64::new(0)),
        sink: Arc::new(|_| {}),
        cancel,
        shared: Arc::new(Mutex::new(Ledger::default())),
    };
    let prompt = spec["prompt"].as_str().context("task requires prompt")?;
    let mut result = runtime
        .run(Some(prompt.into()), vec![], hooks.clone())
        .await?;
    let mut attempts = 1;
    let mut failures = grade(&spec, &before, &runtime, &hooks).await?;
    if baseline == "b2" {
        for _ in 0..spec["max_fix_attempts"].as_u64().unwrap_or(3) {
            if failures.is_empty() || hooks.cancel.is_cancelled() {
                break;
            }
            let budget = &hooks.settings.lock().unwrap().budget.clone();
            if runtime.rounds >= budget.max_rounds
                || runtime.usage.total() >= budget.max_total_tokens as u64
                || started.elapsed().as_secs_f64() >= budget.max_seconds
            {
                break;
            }
            let next = format!(
                "验收尚未通过，请继续修复实现，不得修改受保护文件。\n{}",
                failures.join("\n")
            );
            result = runtime.run(Some(next), vec![], hooks.clone()).await?;
            attempts += 1;
            failures = grade(&spec, &before, &runtime, &hooks).await?;
        }
    }
    if result.exit_reason != "completed" {
        failures.push(format!(
            "agent stopped before completion: {}",
            result.exit_reason
        ));
    }
    let hidden = task.parent().and_then(Path::parent).map(|p| {
        p.join("hidden_tests")
            .join(id.as_ref())
            .join("test_hidden.test.mjs")
    });
    let hidden_pass = if let Some(path) = hidden.filter(|p| p.is_file()) {
        let target = run_dir.join("test_hidden.test.mjs");
        fs::copy(path, &target)?;
        let mut ctx = runtime.context(&hooks, None);
        ctx.deadline = None;
        ctx.cancel = CancellationToken::new();
        let command = format!("node --test \"{}\"", target.display());
        let outcome = tools::run_command(&command, &runtime.workspace, 120.0, &ctx).await?;
        if !outcome.success {
            failures.push(format!(
                "hidden tests failed: {}",
                util::suffix(&outcome.output, 2000)
            ));
        }
        Some(outcome.success)
    } else {
        if spec["require_hidden"] == true {
            failures.push("missing host-side hidden tests".into());
        }
        None
    };
    let events = runtime.store.events(&runtime.session, -1)?;
    let record = json!({"task":id,"task_id":id,"baseline":baseline,"repeat":repeat,"model":model,"fixture_sha256":fixture,"passed":failures.is_empty(),"hidden_pass":hidden_pass,"exit_reason":result.exit_reason,"attempts":attempts,"rounds":runtime.rounds,"usage":runtime.usage,"input_tokens":runtime.usage.input_tokens,"output_tokens":runtime.usage.output_tokens,"seconds":started.elapsed().as_secs_f64(),"error":if failures.is_empty(){Value::Null}else{json!(failures.join("; "))},"session_id":runtime.session,"compactions":events.iter().filter(|e|e.r#type=="context_compacted").count()});
    util::atomic_write(&record_path, &serde_json::to_vec_pretty(&record)?)?;
    Ok(record)
}
pub async fn run(mut options: Options) -> Result<()> {
    if options.repeats == 0 || options.concurrency == 0 || options.concurrency > 32 {
        bail!("repeats and concurrency must be positive (concurrency <=32)")
    }
    if !options.tasks_dir.is_dir() {
        let runtime = std::env::var_os("MINICODE_RESOURCES")
            .map(|p| PathBuf::from(p).join("runtime"))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
            })
            .context("cannot find evaluation fixtures")?;
        options.tasks_dir = runtime
            .join("evals")
            .join(options.tasks_dir.file_name().unwrap_or_default());
    }
    options.tasks_dir = util::canonical(&options.tasks_dir)?;
    options.output = util::canonical(&options.output)?;
    if options.output.starts_with(&options.tasks_dir) {
        bail!("evaluation output must be outside the fixture directory")
    }
    let baselines = options
        .baselines
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>();
    if baselines.iter().any(|b| !["b0", "b1", "b2"].contains(b)) {
        bail!("baselines must be b0, b1 or b2")
    }
    let mut tasks = fs::read_dir(&options.tasks_dir)?
        .filter_map(Result::ok)
        .filter(|e| e.path().join("task.yaml").is_file())
        .filter(|e| {
            options.task.is_empty()
                || options
                    .task
                    .contains(&e.file_name().to_string_lossy().into_owned())
        })
        .map(|e| e.path())
        .collect::<Vec<_>>();
    tasks.sort();
    if tasks.is_empty() {
        bail!("no evaluation tasks found")
    }
    let mut combinations = vec![];
    for task in &tasks {
        for repeat in 1..=options.repeats {
            for baseline in &baselines {
                combinations.push((task.clone(), baseline.to_string(), repeat));
            }
        }
    }
    if options.schedule == "grouped" {
        combinations.sort_by(|a, b| (&a.1, &a.0, a.2).cmp(&(&b.1, &b.0, b.2)));
    }
    let config = json!({"version":crate::VERSION,"model":options.model,"baselines":baselines,"repeats":options.repeats,"concurrency":options.concurrency,"schedule":options.schedule,"fixtures":tasks.iter().map(|p|goals::fingerprint(p).map(|hash|json!({"task":p.file_name().unwrap().to_string_lossy(),"sha256":hash}))).collect::<Result<Vec<_>>>()?});
    let config_path = options.output.join("config.json");
    if config_path.is_file() && serde_json::from_slice::<Value>(&fs::read(&config_path)?)? != config
    {
        bail!("output directory contains a different evaluation configuration")
    }
    util::atomic_write(&config_path, &serde_json::to_vec_pretty(&config)?)?;
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let watcher = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let mut queue = stream::iter(combinations.into_iter().map(|(task, baseline, repeat)| {
        let options = &options;
        let cancel = cancel.clone();
        async move { one(options, &task, &baseline, repeat, cancel).await }
    }))
    .buffer_unordered(options.concurrency);
    let mut results = vec![];
    while let Some(record) = queue.next().await {
        let record = record?;
        println!(
            "{} {} #{}: {}",
            record["task"].as_str().unwrap(),
            record["baseline"].as_str().unwrap(),
            record["repeat"],
            if record["passed"] == true {
                "PASS"
            } else {
                "FAIL"
            }
        );
        results.push(record);
        util::atomic_write(
            &options.output.join("results.json"),
            &serde_json::to_vec_pretty(
                &json!({"run_count":results.len(),"passed":results.iter().filter(|r|r["passed"]==true).count(),"results":results}),
            )?,
        )?;
    }
    watcher.abort();
    let mut summary = String::from(
        "# MiniCode evaluation\n\n| Baseline | Pass / runs | Mean seconds | Mean input tokens |\n| --- | --- | --- | --- |\n",
    );
    for baseline in baselines {
        let records = results
            .iter()
            .filter(|r| r["baseline"] == baseline)
            .collect::<Vec<_>>();
        let n = records.len() as f64;
        summary.push_str(&format!(
            "| {baseline} | {} / {} | {:.3} | {:.1} |\n",
            records.iter().filter(|r| r["passed"] == true).count(),
            records.len(),
            records
                .iter()
                .map(|r| r["seconds"].as_f64().unwrap_or(0.0))
                .sum::<f64>()
                / n,
            records
                .iter()
                .map(|r| r["input_tokens"].as_u64().unwrap_or(0) as f64)
                .sum::<f64>()
                / n
        ));
    }
    util::atomic_write(&options.output.join("summary.md"), summary.as_bytes())?;
    Ok(())
}
