use crate::{
    store::Store,
    tools::{self, ToolContext},
    util,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, path::Path};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub r#type: String,
    pub command: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default = "default_id")]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "attempts")]
    pub max_fix_attempts: usize,
    pub items: Vec<Item>,
}
fn default_id() -> String {
    "goal".into()
}
fn attempts() -> usize {
    3
}
impl Spec {
    pub fn read(path: &Path) -> Result<Self> {
        let source = crate::extensions::Source::read(path, 1_000_000)?;
        let spec: Self = serde_yaml::from_str(&source.content)
            .with_context(|| format!("验收配置校验失败 ({})", path.display()))?;
        spec.validate()?;
        Ok(spec)
    }
    pub fn validate(&self) -> Result<()> {
        if self.items.is_empty() || self.max_fix_attempts < 1 {
            bail!("验收配置至少需要 1 项检查")
        }
        let mut ids = std::collections::HashSet::new();
        for item in &self.items {
            if !ids.insert(&item.id) {
                bail!("检查项 id 重复: {}", item.id)
            }
            let valid = match item.r#type.as_str() {
                "command" => item.command.as_ref().is_some_and(|s| !s.trim().is_empty()),
                "artifact" | "protected" => {
                    item.path.as_ref().is_some_and(|s| !s.trim().is_empty())
                }
                _ => false,
            };
            if !valid {
                bail!("验收配置缺少必填字段或 type 无效: {}", item.id)
            }
        }
        Ok(())
    }
}
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn fingerprint(root: &Path) -> Result<String> {
    let mut entries = vec![];
    let files = walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !util::SKIP_DIRS
                    .iter()
                    .any(|name| *name != ".minicode" && e.file_name() == *name)
        });
    for entry in files
        .filter_map(Result::ok)
        .filter(|e| !e.file_type().is_dir())
    {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let mut hash = Sha256::new();
        hash.update(relative.as_bytes());
        hash.update(b"\0");
        if entry.file_type().is_symlink() {
            hash.update(format!("symlink:{}", fs::read_link(path)?.display()));
            hash.update(b"-1");
        } else {
            let size = path.metadata()?.len();
            hash.update(hash_file(path)?.as_bytes());
            hash.update(size.to_string().as_bytes());
        }
        entries.push(format!("{:x}", hash.finalize()));
    }
    entries.sort();
    Ok(util::hash(entries.join("\n").as_bytes()))
}
#[derive(Clone)]
pub struct Goal {
    pub spec: Spec,
    pub protected: BTreeMap<String, Option<String>>,
    pub unresolved: BTreeMap<String, String>,
    pub attempts: usize,
    pub evidence: Option<Value>,
}
impl Goal {
    pub fn new(spec: Spec, workspace: &Path) -> Result<Self> {
        spec.validate()?;
        let mut protected = BTreeMap::new();
        let mut unresolved = BTreeMap::new();
        for item in spec.items.iter().filter(|i| i.r#type == "protected") {
            let raw = item.path.as_ref().unwrap();
            match util::in_workspace(workspace, raw) {
                Ok(path) => {
                    if path.exists() && !path.is_file() {
                        unresolved.insert(
                            raw.clone(),
                            "protected 检查只支持普通文件，请为目录列出需要保护的文件".into(),
                        );
                    }
                    protected.insert(
                        raw.clone(),
                        if path.is_file() {
                            Some(hash_file(&path)?)
                        } else {
                            None
                        },
                    );
                }
                Err(e) => {
                    protected.insert(raw.clone(), None);
                    unresolved.insert(raw.clone(), e.to_string());
                }
            }
        }
        Ok(Self {
            spec,
            protected,
            unresolved,
            attempts: 0,
            evidence: None,
        })
    }
    pub fn restore(state: &Value, workspace: &Path) -> Result<Self> {
        let spec: Spec = serde_json::from_value(state["spec"].clone())?;
        spec.validate()?;
        let protected = state
            .get("baseline")
            .or_else(|| state.get("protected_snapshot"))
            .or_else(|| state.get("protected"))
            .cloned()
            .unwrap_or(json!({}));
        let mut goal = Self {
            spec,
            protected: serde_json::from_value(protected["digests"].clone()).unwrap_or_default(),
            unresolved: serde_json::from_value(protected["unresolvable"].clone())
                .unwrap_or_default(),
            attempts: state["attempts"].as_u64().unwrap_or(0) as usize,
            evidence: state.get("evidence").cloned(),
        };
        for item in &goal.spec.items {
            if item.r#type == "protected" {
                let path = item.path.as_ref().unwrap();
                if !goal.protected.contains_key(path) {
                    goal.unresolved
                        .insert(path.clone(), "缺少会话初始基线，无法验证受保护文件".into());
                }
            }
        }
        let _ = workspace;
        Ok(goal)
    }
    pub fn save(&self, store: &Store, id: &str) -> Result<()> {
        store.save_goal(id,&json!({"spec":self.spec,"baseline":{"digests":self.protected,"unresolvable":self.unresolved},"attempts":self.attempts,"evidence":self.evidence}))
    }
    pub async fn check(&mut self, ctx: &ToolContext) -> Result<Value> {
        let mut results = BTreeMap::new();
        let ran_at = util::now();
        for item in self.spec.items.iter().filter(|i| i.r#type != "protected") {
            let (passed, detail, exit) = if item.r#type == "command" {
                // Existing Windows acceptance files contain PowerShell commands.
                let shell = if cfg!(windows) { "powershell" } else { "bash" };
                let result = tools::run_command_for(
                    shell,
                    item.command.as_ref().unwrap(),
                    &ctx.workspace,
                    120.0,
                    ctx,
                )
                .await?;
                (result.success, result.output, result.exit_code)
            } else {
                let path = util::in_workspace(&ctx.workspace, item.path.as_ref().unwrap())?;
                (
                    path.is_file(),
                    if path.is_file() {
                        format!("文件存在：{}", path.display())
                    } else {
                        format!("文件不存在：{}", path.display())
                    },
                    None,
                )
            };
            results.insert(item.id.clone(),json!({"item_id":item.id,"kind":item.r#type,"passed":passed,"exit_code":exit,"detail":detail,"ran_at":ran_at}));
        }
        for item in self.spec.items.iter().filter(|i| i.r#type == "protected") {
            let raw = item.path.as_ref().unwrap();
            let (passed, detail) = if let Some(reason) = self.unresolved.get(raw) {
                (false, reason.clone())
            } else {
                let path = util::in_workspace(&ctx.workspace, raw)?;
                let current = if path.is_file() {
                    Some(hash_file(&path)?)
                } else {
                    None
                };
                let expected = self
                    .protected
                    .get(raw)
                    .context("missing protected baseline")?;
                if path.exists() && !path.is_file() {
                    (false, "受保护路径变成了目录或非普通文件".into())
                } else if &current == expected {
                    (true, "受保护文件未发生改变".into())
                } else {
                    (false, "受保护文件内容在快照后被修改".into())
                }
            };
            results.insert(item.id.clone(),json!({"item_id":item.id,"kind":"protected","passed":passed,"detail":detail,"exit_code":null,"ran_at":ran_at}));
        }
        let items = self
            .spec
            .items
            .iter()
            .map(|i| results.remove(&i.id).unwrap())
            .collect::<Vec<_>>();
        let passed = items.iter().all(|i| i["passed"] == true);
        let fingerprint = fingerprint(&ctx.workspace)?;
        let report = json!({"passed":passed,"items":items,"fingerprint":fingerprint,"ran_at":ran_at,"attempt":self.attempts+1});
        if passed {
            self.evidence = Some(report.clone());
        } else {
            self.evidence = None;
            self.attempts += 1;
        }
        self.save(&ctx.store, &ctx.session)?;
        Ok(report)
    }
    pub fn failure(report: &Value) -> String {
        format!(
            "验收尚未通过，请根据实际失败结果继续修复，不得修改受保护文件或虚构证据。\n{}",
            report["items"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter(|i| i["passed"] != true)
                .map(|i| format!(
                    "{}：{}",
                    i["item_id"].as_str().unwrap_or_default(),
                    i["detail"].as_str().unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}
