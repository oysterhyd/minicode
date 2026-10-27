use crate::{configuration::Configuration, util};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub digest: String,
    pub content: String,
}
impl Source {
    pub fn read(path: &Path, limit: u64) -> Result<Self> {
        let mut bytes = vec![];
        fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            bail!("instruction file is too large: {}", path.display())
        }
        Ok(Self {
            path: path.into(),
            digest: util::hash(&bytes),
            content: String::from_utf8(bytes)?,
        })
    }
    pub fn metadata(&self, global: &Path) -> Value {
        json!({"path":self.path,"sha256":self.digest,"scope":if self.path==global{"global"}else{"project"}})
    }
}
#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub origin: String,
}
#[derive(Clone, Debug)]
pub struct Server {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: f64,
    pub plugin: String,
    pub version: String,
    pub digest: String,
    pub source: PathBuf,
    pub transport: String,
    pub url: String,
    pub env: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub local: bool,
    pub configuration_error: String,
}
impl Server {
    pub fn verify(&self) -> Result<()> {
        if !self.configuration_error.is_empty() {
            bail!("{}", self.configuration_error)
        }
        let digest = if self.local {
            util::hash(&fs::read(&self.source)?)
        } else {
            fingerprint(&self.cwd)?
        };
        if digest != self.digest {
            bail!("MCP configuration changed; reload extensions")
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct Catalog {
    pub workspace: PathBuf,
    pub skills: BTreeMap<String, Skill>,
    pub plugins: Vec<Value>,
    pub servers: Vec<Server>,
    pub agents: BTreeMap<String, (Source, Value)>,
    pub local: Vec<Value>,
    pub instructions: BTreeMap<PathBuf, Source>,
    pub active: BTreeMap<String, String>,
    pub pending_scopes: std::collections::HashSet<PathBuf>,
    pub configuration: Configuration,
}
pub(crate) fn compatible_version(required: &str, installed: &str) -> bool {
    fn parse(version: &str) -> Option<[u64; 3]> {
        let mut parts = version.split('.');
        let values = [
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ];
        parts.next().is_none().then_some(values)
    }
    match (parse(required), parse(installed)) {
        (Some(required), Some(installed)) => {
            required[..2] == installed[..2] && required[2] <= installed[2]
        }
        _ => false,
    }
}
fn json_file(path: &Path, limit: u64) -> Result<Value> {
    let source = Source::read(path, limit)?;
    if path.extension().is_some_and(|x| x == "toml") {
        Ok(serde_json::to_value(toml::from_str::<toml::Value>(
            source.content.trim_start_matches('\u{feff}'),
        )?)?)
    } else {
        Ok(serde_json::from_str(
            source.content.trim_start_matches('\u{feff}'),
        )?)
    }
}
pub fn skill_metadata(path: &Path) -> Result<(String, String)> {
    if util::is_link(path) {
        bail!("skill path is a link or junction")
    }
    let mut bytes = vec![];
    fs::File::open(path)?.take(8193).read_to_end(&mut bytes)?;
    // Discovery reads only the header; the body stays on disk until activated.
    let raw = String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let head = raw
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---").map(|p| p.0))
        .context("skill has no YAML front matter")?;
    if head.len() > 8192 || head.lines().count() > 80 {
        bail!("skill header is too large")
    }
    let meta: Value = serde_yaml::from_str(head)?;
    Ok((
        util::string(&meta, "name")?.into(),
        util::string(&meta, "description")?.into(),
    ))
}
pub fn fingerprint(root: &Path) -> Result<String> {
    if util::is_link(root) {
        bail!("plugin directory is a link")
    }
    let mut files = vec![];
    for entry in walkdir::WalkDir::new(root).sort_by_file_name().into_iter() {
        let entry = entry?;
        if util::is_link(entry.path()) {
            bail!("plugin contains a link: {}", entry.path().display())
        }
        if entry.file_type().is_file()
            && !entry
                .path()
                .extension()
                .is_some_and(|s| s == "pyc" || s == "pyo")
        {
            files.push(entry.into_path());
        }
    }
    files.sort();
    if files.len() > 10_000 {
        bail!("plugin has too many files")
    }
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    for path in files {
        if path.components().any(|s| s.as_os_str() == "__pycache__") {
            continue;
        }
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        // Length framing prevents distinct filename/content layouts from
        // producing the same concatenated byte stream before hashing.
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative.as_bytes());
        let mut file = fs::File::open(path)?;
        digest.update(file.metadata()?.len().to_le_bytes());
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > 16_000_000 {
                bail!("plugin content is too large")
            }
            digest.update(&buffer[..n]);
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn directories(root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            if e.path().is_dir() {
                Some(e.path())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}
impl Catalog {
    pub fn scan(workspace: &Path, configuration: Configuration, check_lock: bool) -> Result<Self> {
        let mut result = Self {
            workspace: util::canonical(workspace)?,
            skills: BTreeMap::new(),
            plugins: vec![],
            servers: vec![],
            agents: BTreeMap::new(),
            local: vec![],
            instructions: BTreeMap::new(),
            active: BTreeMap::new(),
            pending_scopes: std::collections::HashSet::new(),
            configuration,
        };
        result.reload_instructions()?;
        let home = util::home();
        for (origin, root) in [
            ("user", home.join(".minicode/skills")),
            ("project", result.workspace.join(".minicode/skills")),
        ] {
            result.skills_at(&root, origin, None)?;
        }
        result.scan_plugins(check_lock)?;
        result.scan_local()?;
        Ok(result)
    }
    pub fn refresh(&self) -> Result<Self> {
        let mut next = Self::scan(&self.workspace, self.configuration.clone(), true)?;
        next.instructions = self.instructions.clone();
        next.reload_instructions()?;
        for (name, digest) in &self.active {
            if next.skills.contains_key(name) && next.load(name)?.digest != *digest {
                bail!("activated skill changed since session start: {name}");
            }
        }
        Ok(next)
    }
    fn skills_at(&mut self, root: &Path, origin: &str, parent: Option<&str>) -> Result<()> {
        for dir in directories(root).into_iter().take(1000) {
            let path = dir.join("SKILL.md");
            if !path.is_file() {
                continue;
            }
            if let Ok((name, description)) = skill_metadata(&path) {
                let key = if origin.starts_with("plugin:") {
                    format!("{origin}:{name}")
                } else {
                    name
                };
                if parent.is_none()
                    && self
                        .skills
                        .get(&key)
                        .is_some_and(|skill| !(origin == "project" && skill.origin == "user"))
                {
                    bail!("duplicate skill name: {key}")
                }
                self.skills.insert(
                    key.clone(),
                    Skill {
                        name: key,
                        description,
                        path: util::canonical(&path)?,
                        root: util::canonical(&dir)?,
                        origin: origin.into(),
                    },
                );
            }
        }
        Ok(())
    }
    fn scan_plugins(&mut self, check_lock: bool) -> Result<()> {
        let root = self.workspace.join(".minicode/plugins");
        if util::is_link(&root) || util::is_link(root.parent().unwrap()) {
            bail!("plugin configuration directory is a link or junction")
        }
        if !root.exists() {
            return Ok(());
        }
        let mut lock = json!({"fingerprint_format":2,"plugins":{}});
        let trust = self.trust_store();
        let trusted_plugins =
            &trust["workspaces"][self.workspace.to_string_lossy().as_ref()]["plugins"];
        static NAME: OnceLock<regex::Regex> = OnceLock::new();
        let valid_name = NAME.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9_]{0,31}$").unwrap());
        for dir in directories(&root) {
            let path = dir.join("plugin.json");
            if !path.is_file() {
                continue;
            }
            let raw = json_file(&path, 64_000)?;
            let name = util::string(&raw, "name")?;
            let version = util::string(&raw, "version")?;
            if !valid_name.is_match(name) || dir.file_name().unwrap_or_default() != name {
                bail!("invalid plugin name")
            }
            if !compatible_version(
                raw["minicode_version"].as_str().unwrap_or_default(),
                crate::VERSION,
            ) {
                bail!(
                    "plugin {name} requires minicode {}; installed {}",
                    raw["minicode_version"],
                    crate::VERSION
                )
            }
            if !raw["enabled"].is_boolean() {
                bail!("plugin enabled must be boolean")
            }
            for key in raw.as_object().context("plugin must be an object")?.keys() {
                if ![
                    "name",
                    "version",
                    "minicode_version",
                    "enabled",
                    "skills",
                    "agents",
                    "mcp_servers",
                    "description",
                ]
                .contains(&key.as_str())
                {
                    bail!("unknown plugin field: {key}")
                }
            }
            let digest = fingerprint(&dir)?;
            lock["plugins"][name] = json!({"version":version,"sha256":digest});
            let enabled = raw["enabled"] == true;
            // Plugin MCP servers are processes chosen by the repository, so they
            // only start once the user trusted this exact plugin fingerprint.
            let trusted = trusted_plugins[name] == digest.as_str();
            let mut server_names = vec![];
            let mut commands = vec![];
            if let Some(servers) = raw["mcp_servers"].as_array() {
                for s in servers {
                    let server = util::string(s, "name")?;
                    server_names.push(json!(server));
                    if !enabled {
                        continue;
                    }
                    let command = util::string(s, "command")?;
                    let timeout = s["timeout_s"].as_f64().unwrap_or(30.0);
                    if !(0.0 < timeout && timeout <= 300.0) {
                        bail!("invalid MCP timeout")
                    }
                    commands.push(json!({"name":server,"command":command,"args":s.get("args").cloned().unwrap_or(json!([]))}));
                    if !trusted {
                        continue;
                    }
                    self.servers.push(Server {
                        name: server.into(),
                        command: if command == "$PYTHON" {
                            "python".into()
                        } else {
                            command.into()
                        },
                        args: serde_json::from_value(s.get("args").cloned().unwrap_or(json!([])))?,
                        cwd: dir.clone(),
                        timeout,
                        plugin: name.into(),
                        version: version.into(),
                        digest: digest.clone(),
                        source: path.clone(),
                        transport: "stdio".into(),
                        url: String::new(),
                        env: HashMap::new(),
                        headers: HashMap::new(),
                        local: false,
                        configuration_error: String::new(),
                    });
                }
            }
            if enabled {
                if let Some(relative) = raw["skills"].as_str() {
                    let skillroot = util::in_workspace(&dir, relative)?;
                    self.skills_at(&skillroot, &format!("plugin:{name}"), None)?;
                }
                if let Some(relative) = raw["agents"].as_str() {
                    let agentroot = util::in_workspace(&dir, relative)?;
                    for folder in directories(&agentroot) {
                        let path = folder.join("AGENT.md");
                        if path.is_file() {
                            let source = Source::read(&path, 64_000)?;
                            let agent = folder.file_name().unwrap_or_default().to_string_lossy();
                            self.agents.insert(format!("plugin:{name}:{agent}"),(source,json!({"plugin":name,"plugin_version":version,"plugin_sha256":digest,"agent":agent})));
                        }
                    }
                }
            }
            self.plugins.push(json!({"name":name,"version":version,"enabled":enabled,"digest":&digest[..12],"sha256":digest,"path":dir,"skills":raw["skills"].is_string(),"agents":raw["agents"].is_string(),"servers":server_names,"trusted":trusted,"trustRequired":!trusted&&!commands.is_empty(),"commands":commands}));
        }
        let lockpath = self.workspace.join(".minicode/plugins.lock.json");
        if check_lock && lockpath.exists() && json_file(&lockpath, 1_000_000)? != lock {
            bail!(
                "plugin lock differs from local manifests; run `minicode plugins lock` after reviewing changes"
            )
        }
        let mut names = std::collections::HashSet::new();
        if self.servers.iter().any(|s| !names.insert(s.name.clone())) {
            bail!("duplicate MCP server name")
        }
        Ok(())
    }
    /// Workspace trust lives in the user directory, never in the workspace.
    fn trust_path(&self) -> PathBuf {
        self.configuration
            .path
            .parent()
            .unwrap_or(Path::new("."))
            .join("trusted-plugins.json")
    }
    fn trust_store(&self) -> Value {
        fs::read(self.trust_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|v| v["workspaces"].is_object())
            .unwrap_or(json!({"workspaces":{}}))
    }
    /// Enabled plugins whose MCP servers wait for the user's trust.
    pub fn trust_required(&self) -> Vec<Value> {
        self.plugins
            .iter()
            .filter(|p| p["trustRequired"] == true)
            .cloned()
            .collect()
    }
    /// Trusts (or revokes) plugins at the fingerprints the user reviewed. A
    /// plugin that changed since then is refused rather than trusted blind.
    pub fn set_trust(&self, reviewed: &[(String, String)], trusted: bool) -> Result<()> {
        let mut store = self.trust_store();
        let workspace = self.workspace.to_string_lossy().into_owned();
        if !store["workspaces"][&workspace]["plugins"].is_object() {
            store["workspaces"][&workspace] = json!({"plugins":{}});
        }
        for (name, digest) in reviewed {
            let Some(plugin) = self.plugins.iter().find(|p| p["name"] == name.as_str()) else {
                bail!("unknown plugin: {name}")
            };
            if trusted {
                if plugin["sha256"] != digest.as_str() {
                    bail!("plugin {name} changed since it was reviewed; review it again")
                }
                store["workspaces"][&workspace]["plugins"][name] = json!(digest);
            } else if let Some(plugins) = store["workspaces"][&workspace]["plugins"].as_object_mut()
            {
                plugins.remove(name);
            }
        }
        util::atomic_write(&self.trust_path(), &serde_json::to_vec_pretty(&store)?)
    }
    pub fn write_lock(&self) -> Result<PathBuf> {
        let mut lock = json!({"fingerprint_format":2,"plugins":{}});
        let root = self.workspace.join(".minicode/plugins");
        for dir in directories(&root) {
            let path = dir.join("plugin.json");
            if path.is_file() {
                let raw = json_file(&path, 64_000)?;
                lock["plugins"][util::string(&raw, "name")?] =
                    json!({"version":raw["version"],"sha256":fingerprint(&dir)?});
            }
        }
        let path = self.workspace.join(".minicode/plugins.lock.json");
        util::atomic_write(&path, &serde_json::to_vec_pretty(&lock)?)?;
        Ok(path)
    }
    pub fn set_plugin(&self, name: &str, enabled: bool) -> Result<()> {
        if !self.plugins.iter().any(|p| p["name"] == name) {
            bail!("unknown plugin: {name}")
        }
        let path = self
            .workspace
            .join(".minicode/plugins")
            .join(name)
            .join("plugin.json");
        let mut raw = json_file(&path, 64_000)?;
        raw["enabled"] = json!(enabled);
        util::atomic_write(&path, &serde_json::to_vec_pretty(&raw)?)?;
        self.write_lock()?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // One complete resource metadata record.
    fn local_entry(
        &mut self,
        kind: &str,
        path: &Path,
        name: &str,
        description: &str,
        scope: &str,
        source: &str,
        supported: bool,
        detail: &str,
        default: bool,
        parent: &str,
    ) -> Result<String> {
        let canonical = util::canonical(path)?;
        let key = format!(
            "{kind}:{}",
            &util::hash(format!("{}\0{name}{parent}", canonical.display()).as_bytes())[..24]
        );
        if self.local.iter().any(|e| e["id"] == key) {
            return Ok(key);
        }
        let data = self.configuration.read()?;
        let selected = if parent.is_empty() { &key } else { parent };
        let enabled = data["localExtensions"]
            .get(selected)
            .map(|v| v == true)
            .unwrap_or(default)
            && supported;
        self.local.push(json!({"id":key,"kind":kind,"name":name,"description":util::prefix(description,1000),"scope":scope,"source":source,"path":path,"supported":supported,"status":if supported{"available"}else{"unsupported"},"detail":detail,"enabled":enabled,"parentId":parent}));
        Ok(key)
    }
    fn local_skills(&mut self, root: &Path, scope: &str, source: &str, parent: &str) -> Result<()> {
        let paths = if root.join("SKILL.md").is_file() {
            vec![root.join("SKILL.md")]
        } else {
            directories(root)
                .into_iter()
                .map(|d| d.join("SKILL.md"))
                .filter(|p| p.is_file())
                .take(1000)
                .collect()
        };
        for path in paths {
            match skill_metadata(&path) {
                Ok((name, description)) => {
                    let key = self.local_entry(
                        "skill",
                        &path,
                        &name,
                        &description,
                        scope,
                        source,
                        true,
                        "",
                        source == "MiniCode",
                        parent,
                    )?;
                    let enabled = self
                        .local
                        .iter()
                        .find(|e| e["id"] == key)
                        .is_some_and(|e| e["enabled"] == true);
                    if source == "MiniCode" {
                        if !enabled {
                            self.skills.remove(&name);
                        }
                    } else if enabled {
                        let name =
                            format!("local:{}:{name}", &key.split(':').nth(1).unwrap()[..12]);
                        let path = util::canonical(&path)?;
                        self.skills.insert(
                            name.clone(),
                            Skill {
                                name,
                                description,
                                root: path.parent().unwrap().into(),
                                path,
                                origin: format!("local:{source}:{scope}"),
                            },
                        );
                    }
                }
                Err(_) => {
                    self.local_entry(
                        "skill",
                        &path,
                        &path
                            .parent()
                            .unwrap_or(root)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        "",
                        scope,
                        source,
                        false,
                        "Invalid skill metadata",
                        false,
                        parent,
                    )?;
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Source identity and parent-plugin provenance.
    fn local_mcp(
        &mut self,
        path: &Path,
        scope: &str,
        source: &str,
        parent: &str,
        individual: bool,
        override_raw: Option<Value>,
        plugin_root: &Path,
    ) -> Result<()> {
        if !path.is_file() {
            return Ok(());
        }
        let raw = match override_raw
            .map(Ok)
            .unwrap_or_else(|| json_file(path, 1_000_000))
        {
            Ok(v) => v,
            Err(_) => {
                self.local_entry(
                    "mcp",
                    path,
                    &path.file_stem().unwrap_or_default().to_string_lossy(),
                    "",
                    scope,
                    source,
                    false,
                    "Invalid configuration",
                    false,
                    parent,
                )?;
                return Ok(());
            }
        };
        let values = if individual {
            json!({raw["id"].as_str().unwrap_or(path.file_stem().unwrap_or_default().to_str().unwrap_or("server")):raw})
        } else {
            raw.get("mcpServers")
                .or_else(|| raw.get("mcp_servers"))
                .or_else(|| raw.get("servers"))
                .cloned()
                .unwrap_or(json!({}))
        };
        static READABLE: OnceLock<regex::Regex> = OnceLock::new();
        let readable_pattern = READABLE.get_or_init(|| regex::Regex::new("[^a-z0-9_]").unwrap());
        static ENVIRONMENT: OnceLock<regex::Regex> = OnceLock::new();
        let environment_pattern = ENVIRONMENT
            .get_or_init(|| regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap());
        for (name, s) in values
            .as_object()
            .unwrap_or(&serde_json::Map::new())
            .iter()
            .take(256)
        {
            let transport = s
                .get("transport")
                .or_else(|| s.get("type"))
                .and_then(Value::as_str)
                .unwrap_or(if s["url"].is_string() {
                    "http"
                } else {
                    "stdio"
                });
            let transport = if ["streamable-http", "streamable_http"].contains(&transport) {
                "http"
            } else {
                transport
            };
            let args =
                serde_json::from_value::<Vec<String>>(s.get("args").cloned().unwrap_or(json!([])));
            let valid = ["env", "headers"].iter().all(|field| {
                s[*field].is_null()
                    || s[*field]
                        .as_object()
                        .is_some_and(|v| v.values().all(Value::is_string))
            }) && args.is_ok()
                && match transport {
                    "stdio" => s["command"].as_str().is_some_and(|s| !s.is_empty()),
                    "http" | "sse" => s["url"].as_str().is_some_and(|s| {
                        reqwest::Url::parse(s).is_ok_and(|u| {
                            ["http", "https"].contains(&u.scheme())
                                && !u.host_str().unwrap_or_default().is_empty()
                                && u.username().is_empty()
                                && u.password().is_none()
                        })
                    }),
                    _ => false,
                };
            let label = s["label"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(name);
            let key = self.local_entry(
                "mcp",
                path,
                label,
                s["description"].as_str().unwrap_or(""),
                scope,
                source,
                valid,
                transport,
                false,
                parent,
            )?;
            let readable = readable_pattern
                .replace_all(&name.to_lowercase(), "_")
                .chars()
                .take(16)
                .collect::<String>();
            let runtime = format!("local_{readable}_{}", &key.split(':').nth(1).unwrap()[..6]);
            let entry = self.local.iter_mut().find(|e| e["id"] == key).unwrap();
            entry["runtimeName"] = json!(runtime);
            if entry["enabled"] != true {
                continue;
            }
            let errors = std::cell::RefCell::new(Vec::new());
            let expand = |text: &str| -> Result<String> {
                let re = environment_pattern;
                let mut value = text.to_string();
                for captures in re.captures_iter(text) {
                    let name = &captures[1];
                    let replacement = if ["CLAUDE_PLUGIN_ROOT", "CODEX_PLUGIN_ROOT", "PLUGIN_ROOT"]
                        .contains(&name)
                    {
                        Some(plugin_root.to_string_lossy().into_owned())
                    } else {
                        std::env::var(name).ok()
                    };
                    if let Some(replacement) = replacement {
                        value = value.replace(&captures[0], &replacement);
                    } else {
                        errors
                            .borrow_mut()
                            .push(format!("MCP requires environment variable {name}"));
                    }
                }
                Ok(value)
            };
            let command = expand(s["command"].as_str().unwrap_or_default())?;
            let cwd = s["cwd"]
                .as_str()
                .map(expand)
                .transpose()?
                .map(PathBuf::from)
                .unwrap_or(self.workspace.clone());
            let cwd = if cwd.is_absolute() {
                cwd
            } else {
                path.parent().unwrap_or(Path::new(".")).join(cwd)
            };
            let env = serde_json::from_value::<HashMap<String, String>>(
                s.get("env").cloned().unwrap_or(json!({})),
            )?
            .into_iter()
            .map(|(k, v)| Ok((k, expand(&v)?)))
            .collect::<Result<_>>()?;
            let headers = serde_json::from_value::<HashMap<String, String>>(
                s.get("headers").cloned().unwrap_or(json!({})),
            )?
            .into_iter()
            .map(|(k, v)| Ok((k, expand(&v)?)))
            .collect::<Result<_>>()?;
            self.servers.push(Server {
                name: runtime,
                command,
                args: args?.iter().map(|a| expand(a)).collect::<Result<_>>()?,
                cwd,
                timeout: 30.0,
                plugin: format!("local:{source}"),
                version: "local".into(),
                digest: util::hash(&fs::read(path)?),
                source: path.into(),
                transport: transport.into(),
                url: expand(s["url"].as_str().unwrap_or_default())?,
                env,
                headers,
                local: true,
                configuration_error: errors.into_inner().join("; "),
            });
        }
        Ok(())
    }
    fn foreign_plugin(&mut self, root: &Path, scope: &str, source: &str) -> Result<()> {
        let path = [
            root.join(".claude-plugin/plugin.json"),
            root.join(".codex-plugin/plugin.json"),
            root.join("manifest.json"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        let Some(path) = path else { return Ok(()) };
        let Ok(raw) = json_file(&path, 1_000_000) else {
            return Ok(());
        };
        let name = raw["name"].as_str().unwrap_or(
            root.file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or("plugin"),
        );
        let key = self.local_entry(
            "plugin",
            &path,
            name,
            raw["description"].as_str().unwrap_or_default(),
            scope,
            source,
            true,
            "",
            false,
            "",
        )?;
        let skills = raw.get("skills").cloned().unwrap_or(json!("./skills"));
        let roots = if let Some(s) = skills.as_str() {
            vec![s.to_string()]
        } else {
            serde_json::from_value::<Vec<String>>(skills).unwrap_or_default()
        };
        for relative in roots {
            if let Ok(p) = util::in_workspace(root, &relative) {
                self.local_skills(&p, scope, source, &key)?;
            }
        }
        self.local_mcp(
            &root.join(".mcp.json"),
            scope,
            source,
            &key,
            false,
            None,
            root,
        )?;
        if let Some(mcp) = raw
            .get("mcpServers")
            .or_else(|| raw["contributes"].get("mcpServers"))
        {
            if mcp.is_object() {
                self.local_mcp(
                    &path,
                    scope,
                    source,
                    &key,
                    false,
                    Some(json!({"mcpServers":mcp})),
                    root,
                )?;
            } else if let Some(relative) = mcp.as_str() {
                if let Ok(p) = util::in_workspace(root, relative) {
                    self.local_mcp(&p, scope, source, &key, false, None, root)?;
                }
            } else if let Some(servers) = mcp.as_array() {
                let mut map = serde_json::Map::new();
                for (i, s) in servers.iter().enumerate() {
                    map.insert(
                        s["id"]
                            .as_str()
                            .or(s["name"].as_str())
                            .map(str::to_string)
                            .unwrap_or(i.to_string()),
                        s.clone(),
                    );
                }
                self.local_mcp(
                    &path,
                    scope,
                    source,
                    &key,
                    false,
                    Some(json!({"mcpServers":map})),
                    root,
                )?;
            }
        }
        let supported = self
            .local
            .iter()
            .any(|e| e["parentId"] == key && e["supported"] == true);
        let entry = self.local.iter_mut().find(|e| e["id"] == key).unwrap();
        entry["supported"] = json!(supported);
        if !supported {
            entry["enabled"] = json!(false);
            entry["status"] = json!("unsupported");
            entry["detail"] = json!("No compatible declarative skills or MCP servers");
        }
        Ok(())
    }
    fn scan_local(&mut self) -> Result<()> {
        let home = util::home();
        let workspace = self.workspace.clone();
        let mut scopes = vec![("global", home.clone())];
        if workspace != home {
            scopes.push(("project", workspace));
        }
        for (scope, base) in scopes {
            for (relative, source) in [
                (".minicode/skills", "MiniCode"),
                (".agents/skills", "Agents"),
                (".claude/skills", "Claude"),
                (".codex/skills", "Codex"),
                (".pi/skills", "Pi"),
                (".pi/agent/skills", "Pi"),
            ] {
                self.local_skills(&base.join(relative), scope, source, "")?;
            }
            for e in fs::read_dir(base.join(".agents/servers"))
                .into_iter()
                .flatten()
                .flatten()
                .take(256)
            {
                if e.path().extension().is_some_and(|x| x == "json") {
                    self.local_mcp(&e.path(), scope, "Agents", "", true, None, Path::new(""))?;
                }
            }
            for (relative, source) in [
                (".mcp.json", "Claude"),
                (".claude.json", "Claude"),
                (".claude/settings.json", "Claude"),
                (".codex/config.toml", "Codex"),
                (".pi/mcp.json", "Pi"),
                (".pi/agent/mcp.json", "Pi"),
                (".cursor/mcp.json", "Cursor"),
            ] {
                self.local_mcp(
                    &base.join(relative),
                    scope,
                    source,
                    "",
                    false,
                    None,
                    Path::new(""),
                )?;
            }
            for (relative, source) in [
                (".codex/plugins/cache", "Codex"),
                (".claude/plugins/cache", "Claude"),
            ] {
                for a in directories(&base.join(relative)) {
                    for b in directories(&a) {
                        for c in directories(&b).into_iter().take(1000) {
                            self.foreign_plugin(&c, scope, source)?;
                        }
                    }
                }
            }
            if let Ok(index) = json_file(
                &base.join(".claude/plugins/installed_plugins.json"),
                1_000_000,
            ) && let Some(plugins) = index["plugins"].as_object()
            {
                for installs in plugins.values() {
                    if let Some(installs) = installs.as_array() {
                        for install in installs {
                            if let Some(path) = install["installPath"].as_str() {
                                self.foreign_plugin(Path::new(path), scope, "Claude")?;
                            }
                        }
                    }
                }
            }
            for relative in [".agents/plugins", ".pi-desktop/plugins"] {
                for root in directories(&base.join(relative)) {
                    self.foreign_plugin(&root, scope, "Pi Desktop")?;
                    for inner in directories(&root) {
                        self.foreign_plugin(&inner, scope, "Pi Desktop")?;
                    }
                }
            }
        }
        if let Ok(override_dir) = std::env::var("PI_DESKTOP_AGENTS_DIR") {
            let dir = PathBuf::from(override_dir);
            self.local_skills(&dir.join("skills"), "global", "Pi Desktop", "")?;
            for e in fs::read_dir(dir.join("servers"))
                .into_iter()
                .flatten()
                .flatten()
                .take(256)
            {
                self.local_mcp(
                    &e.path(),
                    "global",
                    "Pi Desktop",
                    "",
                    true,
                    None,
                    Path::new(""),
                )?;
            }
        }
        let desktop = if cfg!(windows) {
            home.join("AppData/Roaming/Claude/claude_desktop_config.json")
        } else if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Claude/claude_desktop_config.json")
        } else {
            home.join(".config/Claude/claude_desktop_config.json")
        };
        self.local_mcp(
            &desktop,
            "global",
            "Claude Desktop",
            "",
            false,
            None,
            Path::new(""),
        )?;
        self.local.sort_by_key(|e| {
            (
                e["kind"].as_str().unwrap_or_default().to_string(),
                e["scope"].as_str().unwrap_or_default().to_string(),
                e["name"].as_str().unwrap_or_default().to_string(),
                e["id"].as_str().unwrap_or_default().to_string(),
            )
        });
        Ok(())
    }
    pub fn set_local(&self, key: &str, enabled: bool) -> Result<()> {
        let entry = self
            .local
            .iter()
            .find(|e| e["id"] == key)
            .context("local resource is no longer available")?;
        if entry["parentId"] != "" {
            bail!("activate plugin resources using the parent plugin")
        }
        if enabled && entry["supported"] != true {
            bail!("resource is incompatible with MiniCode")
        }
        let mut data = self.configuration.read()?;
        if !data["localExtensions"].is_object() {
            data["localExtensions"] = json!({});
        }
        data["localExtensions"][key] = json!(enabled);
        self.configuration.write(&data)
    }
    pub fn reload_instructions(&mut self) -> Result<Vec<Value>> {
        let global = self
            .configuration
            .path
            .parent()
            .unwrap_or(Path::new("."))
            .join("AGENTS.md");
        let mut changed = vec![];
        for path in [global.clone(), self.workspace.join("AGENTS.md")] {
            if util::is_link(&path) || (path == global && util::is_link(path.parent().unwrap())) {
                bail!("instructions path is a link or junction")
            }
            if path != global && self.instructions.contains_key(&path) {
                continue;
            }
            if path.is_file() {
                let source = Source::read(&path, 64_000)?;
                if self
                    .instructions
                    .get(&path)
                    .is_none_or(|previous| previous.digest != source.digest)
                {
                    changed.push(source.metadata(&global));
                }
                self.instructions.insert(path, source);
            } else {
                self.instructions.remove(&path);
            }
        }
        Ok(changed)
    }
    pub fn discover_instructions(&mut self, target: &str) -> Result<Vec<Value>> {
        let path = util::in_workspace(&self.workspace, target)?;
        if self
            .pending_scopes
            .iter()
            .any(|scope| path.starts_with(scope))
        {
            bail!("该目录的新 AGENTS.md 指令刚被加载；请按新指令重新调用工具。")
        }
        let path = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(&self.workspace).to_path_buf()
        };
        let mut cursor = self.workspace.clone();
        let mut found = vec![];
        for component in path.strip_prefix(&self.workspace)?.components() {
            cursor.push(component);
            let file = cursor.join("AGENTS.md");
            if util::is_link(&file) {
                bail!("instructions path is a link or junction");
            }
            if file.is_file() && !self.instructions.contains_key(&file) {
                let s = Source::read(&file, 64_000)?;
                self.pending_scopes
                    .insert(file.parent().unwrap().to_path_buf());
                found.push(json!({"path":file,"sha256":s.digest,"scope":"project"}));
                self.instructions.insert(file, s);
            }
        }
        Ok(found)
    }
    pub fn load(&mut self, name: &str) -> Result<Source> {
        let s = self.skills.get(name).context("unknown skill")?;
        let source = Source::read(&s.path, 64_000)?;
        if self
            .active
            .get(name)
            .is_some_and(|old| *old != source.digest)
        {
            bail!("skill changed during this session; start a new session: {name}")
        }
        if util::canonical(&s.path)?.parent() != Some(s.root.as_path()) {
            bail!("skill path changed")
        };
        self.active.insert(name.into(), source.digest.clone());
        Ok(source)
    }
    pub fn unload(&mut self, name: &str) -> Result<()> {
        if self.active.remove(name).is_none() {
            bail!("skill is not active: {name}")
        }
        Ok(())
    }
    pub fn resource(&self, name: &str, relative: &str) -> Result<Source> {
        let skill = self.skills.get(name).context("unknown skill")?;
        Source::read(&util::in_workspace(&skill.root, relative)?, 64_000)
    }
    pub fn listing(&self) -> String {
        if self.skills.is_empty() {
            "（未发现 Skills）".into()
        } else {
            self.skills
                .values()
                .map(|s| {
                    format!(
                        "- {}: {} [{}]",
                        s.name,
                        util::prefix(&s.description, 240),
                        s.origin
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
    pub fn prompt(&self, tools: &[String]) -> Result<String> {
        let template = if cfg!(windows) {
            include_str!("assets/system-windows.txt")
        } else {
            include_str!("assets/system-posix.txt")
        };
        let mut text = template
            .replace("{{workspace}}", &self.workspace.to_string_lossy())
            .replace("{{tools}}", &tools.join("、"));
        let global = self
            .configuration
            .path
            .parent()
            .unwrap_or(Path::new("."))
            .join("AGENTS.md");
        let root = self.workspace.join("AGENTS.md");
        for source in self
            .instructions
            .values()
            .filter(|source| source.path == global || source.path == root)
        {
            text.push_str(&format!(
                "\n指令来源：{} (sha256:{})\n{}\n",
                source.path.display(),
                &source.digest[..12],
                source.content
            ));
        }
        text.push_str(&format!(
            "\n可用 Skills（按需用 skill_load 加载）：\n{}\n",
            util::prefix(&self.listing(), 8000)
        ));
        text.push_str("\n运行时会通过 <system-reminder> 更新动态项目指令和 Skills；同一来源以最新有效版本为准。\n");
        Ok(text)
    }
}
