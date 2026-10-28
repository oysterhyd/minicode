use crate::util;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Clone)]
pub struct Configuration {
    pub path: PathBuf,
}

pub fn timeout_seconds(value: &Value) -> Result<f64> {
    let seconds = value.as_f64().context("tool timeout must be a number")?;
    if !seconds.is_finite() || !(0.0 < seconds && seconds <= 3600.0) {
        bail!("tool timeout must be greater than 0 and at most 3600 seconds")
    }
    Ok(seconds)
}

pub fn catalog_model(name: &str) -> Value {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let catalog =
        CATALOG.get_or_init(|| serde_json::from_str(include_str!("assets/models.json")).unwrap());
    let canonical = name
        .strip_prefix("anthropic/")
        .or_else(|| name.strip_prefix("openai/"))
        .unwrap_or(name);
    let dated_alias = canonical.rsplit_once('-').and_then(|(base, date)| {
        (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit())).then_some(base)
    });
    catalog
        .get(name)
        .or_else(|| catalog.get(canonical))
        .or_else(|| dated_alias.and_then(|base| catalog.get(base)))
        .cloned()
        .unwrap_or(Value::Null)
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            path: util::data_dir().join("desktop-config.json"),
        }
    }
}
impl Configuration {
    pub fn read(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(json!({"services":[],"agents":[],"disabledAgents":[],"defaultModel":""}));
        }
        if self.path.metadata()?.len() > 4_000_000 {
            bail!("harness configuration exceeds the size limit")
        }
        let bytes = fs::read(&self.path)?;
        let value: Value =
            serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))?;
        if !value.is_object() || !value["services"].is_array() || !value["agents"].is_array() {
            bail!("harness configuration must contain services and agents lists")
        }
        if value["services"].as_array().unwrap().iter().any(|s| {
            !s.is_object()
                || !s["models"].is_array()
                || s["models"]
                    .as_array()
                    .is_some_and(|models| models.iter().any(|m| !m.is_object()))
        }) || value["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| !a.is_object())
        {
            bail!("invalid service or agent configuration")
        }
        if let Some(timeouts) = value.get("toolTimeouts") {
            for timeout in timeouts
                .as_object()
                .context("toolTimeouts must be an object")?
                .values()
            {
                timeout_seconds(timeout)?;
            }
        }
        Ok(value)
    }
    pub fn tool_timeout(&self, name: &str) -> Result<Option<std::time::Duration>> {
        Ok(self.read()?["toolTimeouts"][name]
            .as_f64()
            .map(std::time::Duration::from_secs_f64))
    }
    pub fn write(&self, data: &Value) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(data)?;
        if bytes.len() > 4_000_000 {
            bail!("harness configuration exceeds the size limit");
        }
        util::atomic_write(&self.path, &bytes)
    }
    pub fn model_key(service: &Value, model: &Value) -> String {
        let name = model["modelId"].as_str().unwrap_or_default();
        if service["builtin"] == true && catalog_model(name)["provider"] == service["id"] {
            name.into()
        } else {
            format!("{}::{name}", service["id"].as_str().unwrap_or_default())
        }
    }
    pub fn credentials(&self, s: &Value) -> (String, String) {
        let url = s["baseUrl"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let mut key = s["apiKey"].as_str().unwrap_or_default().trim().to_string();
        if key.is_empty() {
            key = match s["id"].as_str().unwrap_or_default() {
                "anthropic" => std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
                "commandcode" => discover_commandcode()
                    .map(|(_, key)| key)
                    .unwrap_or_default(),
                _ => String::new(),
            };
        }
        (url, key.trim().to_string())
    }
    pub fn public(&self) -> Result<Value> {
        let mut data = self.read()?;
        for s in data["services"].as_array_mut().unwrap() {
            s["hasApiKey"] = json!(!self.credentials(s).1.is_empty());
            s["apiKey"] = json!("");
            if let Some(models) = s["models"].as_array_mut() {
                for m in models {
                    m["capabilities"] = capabilities(&m["capabilities"])?;
                }
            }
        }
        data["agents"] = json!(self.agents()?);
        Ok(data)
    }
    pub fn models(&self) -> Result<Vec<Value>> {
        let data = self.read()?;
        let mut models = vec![];
        for s in data["services"].as_array().unwrap() {
            for m in s["models"].as_array().context("invalid service models")? {
                models.push(json!({"id":Self::model_key(s,m),"name":m.get("name").filter(|v|v.as_str()!=Some("")).unwrap_or(&m["modelId"]),"modelId":m["modelId"],"serviceId":s["id"],"provider":s["name"],"supportsEffort":m["supportsEffort"],"contextWindow":m["contextWindow"],"maxOutputTokens":m["maxOutputTokens"],"capabilities":capabilities(&m["capabilities"])?,"available":s["enabled"]==true && !self.credentials(s).1.is_empty()}));
            }
        }
        Ok(models)
    }
    pub fn find_model(&self, key: &str) -> Result<Option<(Value, Value)>> {
        let data = self.read()?;
        for s in data["services"].as_array().unwrap() {
            for m in s["models"].as_array().context("invalid models")? {
                if Self::model_key(s, m) == key {
                    return Ok(Some((s.clone(), m.clone())));
                }
            }
        }
        Ok(None)
    }
    pub fn default_model(&self) -> Result<String> {
        let data = self.read()?;
        let default = data["defaultModel"].as_str().unwrap_or_default();
        Ok(if !default.is_empty() {
            default.into()
        } else {
            self.models()?
                .iter()
                .find(|m| m["available"] == true)
                .and_then(|m| m["id"].as_str())
                .unwrap_or_default()
                .into()
        })
    }
    pub fn save_service(&self, draft: &Value) -> Result<Value> {
        if !draft.is_object() {
            bail!("service must be an object")
        }
        let mut data = self.read()?;
        let existing = data["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == draft["id"]);
        let mut s = draft.clone();
        s["id"] = existing
            .map(|s| s["id"].clone())
            .unwrap_or_else(|| json!(util::id()));
        s["builtin"] = json!(existing.is_some_and(|s| s["builtin"] == true));
        s["apiKey"] = json!(s["apiKey"].as_str().unwrap_or_default().trim());
        if s["apiKey"].as_str().unwrap_or_default().is_empty() {
            s["apiKey"] = existing.map(|s| s["apiKey"].clone()).unwrap_or(json!(""));
        }
        s.as_object_mut()
            .context("service must be an object")?
            .remove("hasApiKey");
        let name = util::string(&s, "name")?.trim().to_string();
        let url = util::string(&s, "baseUrl")?
            .trim()
            .trim_end_matches('/')
            .to_string();
        let parsed =
            reqwest::Url::parse(&url).context("请填写服务名称和有效的 HTTP(S) 接口地址")?;
        if name.is_empty()
            || !["http", "https"].contains(&parsed.scheme())
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            bail!("请填写服务名称和有效的 HTTP(S) 接口地址")
        }
        s["name"] = json!(name);
        s["baseUrl"] = json!(url);
        let style = s["apiStyle"].as_str().unwrap_or_default().to_string();
        if !["openai", "anthropic"].contains(&style.as_str()) {
            bail!("不支持的接口格式")
        }
        let mut seen = std::collections::HashSet::new();
        for m in s["models"]
            .as_array_mut()
            .context("models must be an array")?
        {
            let id = util::string(m, "modelId")?.trim().to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                bail!("模型 ID 不能为空或重复")
            }
            m["modelId"] = json!(id);
            let window = m["contextWindow"]
                .as_u64()
                .context("invalid contextWindow")?;
            let output = m["maxOutputTokens"]
                .as_u64()
                .context("invalid maxOutputTokens")?;
            if output == 0 || output >= window {
                bail!("输出上限必须大于 0 且小于上下文窗口")
            }
            m["supportsEffort"] = json!(m["supportsEffort"] == true);
            m["capabilities"] = capabilities(&m["capabilities"])?;
            if style == "anthropic"
                && (m["capabilities"]["audio"] == true || m["capabilities"]["video"] == true)
            {
                bail!("Anthropic Messages 可配置图片与 PDF；音频、视频需要支持这些输入的兼容接口")
            }
        }
        s["enabled"] = json!(s.get("enabled").map(|x| x == true).unwrap_or(true));
        data["services"]
            .as_array_mut()
            .unwrap()
            .retain(|other| other["id"] != s["id"]);
        data["services"].as_array_mut().unwrap().push(s);
        self.write(&data)?;
        let models = self.models()?;
        if !models
            .iter()
            .any(|m| m["id"] == data["defaultModel"] && m["available"] == true)
        {
            data["defaultModel"] = models
                .iter()
                .find(|m| m["available"] == true)
                .map(|m| m["id"].clone())
                .unwrap_or(json!(""));
            self.write(&data)?;
        }
        self.public()
    }
    pub fn agents(&self) -> Result<Vec<Value>> {
        let data = self.read()?;
        let builtins: Vec<Value> = serde_json::from_str(include_str!("assets/agents.json"))?;
        let mut result = vec![];
        for (builtin, items) in [
            (true, builtins),
            (false, data["agents"].as_array().unwrap().clone()),
        ] {
            for mut a in items {
                a["builtin"] = json!(builtin);
                a["inheritTools"] = json!(a["inheritTools"] == true);
                a["enabled"] = json!(
                    !data["disabledAgents"]
                        .as_array()
                        .is_some_and(|names| names.contains(&a["name"]))
                );
                result.push(a);
            }
        }
        Ok(result)
    }
    pub fn save_agent(&self, draft: &Value) -> Result<Vec<Value>> {
        let mut data = self.read()?;
        let name = util::string(draft, "name")?.trim();
        if !regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$")?.is_match(name)
            || self
                .agents()?
                .iter()
                .any(|a| a["builtin"] == true && a["name"] == name)
        {
            bail!("名称需为 1–64 位字母、数字、连字符或下划线，不能与内置子助手重复")
        }
        let original = draft["originalName"].as_str();
        if data["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == name && a["name"].as_str() != original)
        {
            bail!("该子助手名称已存在")
        }
        let description = util::string(draft, "description")?;
        let instructions = util::string(draft, "instructions")?;
        if description.trim().is_empty() || instructions.trim().is_empty() {
            bail!("请填写委派条件和指令")
        }
        if instructions.len() > 32_000 {
            bail!("指令不能超过 32 KB")
        }
        let tools = draft["tools"].as_array().context("invalid tools")?;
        if tools.iter().any(|t| {
            ![
                "read",
                "ls",
                "grep",
                "read_artifact",
                "bash",
                "powershell",
                "edit",
                "write",
            ]
            .contains(&t.as_str().unwrap_or_default())
        }) {
            bail!("未知工具")
        }
        let a = json!({"name":name,"label":draft.get("label").filter(|v|v.as_str()!=Some("")).unwrap_or(&draft["name"]),"description":description,"instructions":instructions,"tools":tools,"inheritTools":draft["inheritTools"]==true});
        data["agents"]
            .as_array_mut()
            .unwrap()
            .retain(|a| a["name"].as_str() != original);
        data["agents"].as_array_mut().unwrap().push(a);
        if let Some(names) = data["disabledAgents"].as_array_mut() {
            names.retain(|a| a.as_str() != original);
        }
        self.write(&data)?;
        self.agents()
    }
}
pub fn capabilities(v: &Value) -> Result<Value> {
    let mut result = json!({"images":false,"pdf":false,"audio":false,"video":false});
    if v.is_null() {
        return Ok(result);
    }
    let object = v.as_object().context("capabilities must be an object")?;
    for key in ["images", "pdf", "audio", "video"] {
        if let Some(value) = object.get(key) {
            if !value.is_boolean() {
                bail!("{key} must be a boolean")
            }
            result[key] = value.clone();
        }
    }
    Ok(result)
}
pub fn discover_commandcode() -> Option<(String, String)> {
    if let Ok(key) = std::env::var("COMMANDCODE_API_KEY")
        && !key.trim().is_empty()
    {
        let url = std::env::var("COMMANDCODE_BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("https://api.commandcode.ai/provider/v1".into());
        return Some((url.trim().trim_end_matches('/').into(), key.trim().into()));
    }
    let data: Value = serde_json::from_slice(
        &fs::read(util::home().join(".zcode/v2/provider_config.json")).ok()?,
    )
    .ok()?;
    let rules = data["config"]["providerConfigRules"]["providerRules"].as_array()?;
    let chosen = rules
        .iter()
        .find(|r| r["providerName"] == "Command Code")
        .or_else(|| {
            rules
                .iter()
                .find(|r| r["config"]["api"]["type"] == "openai-chat-completions")
        })?;
    let url = chosen["config"]["api"]["baseUrl"]
        .as_str()?
        .trim()
        .trim_end_matches('/');
    let key = chosen["config"]["access"]["apiKey"].as_str()?.trim();
    if url.is_empty() || key.is_empty() {
        None
    } else {
        Some((url.into(), key.into()))
    }
}
