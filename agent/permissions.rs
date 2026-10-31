use crate::util;
use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Deny,
    Ask,
    Allow,
}
#[derive(Clone, Default)]
pub struct Rules {
    entries: Vec<Rule>,
}
#[derive(Clone)]
struct Rule {
    effect: Decision,
    tool: GlobMatcher,
    scope: Option<String>,
}

impl Rules {
    pub fn has_file_denies(&self) -> bool {
        self.entries.iter().any(|rule| {
            rule.effect == Decision::Deny
                && ["read", "edit", "write", "grep", "ls"]
                    .iter()
                    .any(|name| rule.tool.is_match(name))
        })
    }
    pub fn load(
        workspace: &Path,
        settings: &Path,
        cli: &Value,
        session: &std::collections::HashSet<String>,
    ) -> Result<Self> {
        let mut rules = Self::default();
        for path in [
            settings.join("settings.json"),
            workspace.join(".minicode/settings.json"),
            workspace.join(".minicode/settings.local.json"),
        ] {
            if path.try_exists()? {
                if util::is_link(&path) {
                    bail!("refusing linked permission settings")
                }
                if path.metadata()?.len() > 1024 * 1024 {
                    bail!("permission settings exceed 1 MiB")
                }
                let value: Value = serde_json::from_slice(&fs::read(&path)?)
                    .with_context(|| format!("invalid permission settings: {}", path.display()))?;
                rules.extend(&value["permissions"])?;
            }
        }
        rules.extend(cli)?;
        for text in session {
            // Legacy whole-tool grants are accepted for native non-shell tools
            // only; shell and MCP grants must now carry a concrete scope.
            if text == "bash" || text == "powershell" {
                continue;
            }
            rules.entries.push(parse(text, Decision::Allow)?);
        }
        Ok(rules)
    }
    pub fn extend(&mut self, value: &Value) -> Result<()> {
        if value.is_null() {
            return Ok(());
        }
        if !value.is_object() {
            bail!("permissions must be an object")
        }
        if value
            .as_object()
            .unwrap()
            .keys()
            .any(|key| !["allow", "ask", "deny"].contains(&key.as_str()))
        {
            bail!("unknown permission effect; expected allow, ask or deny")
        }
        for (key, effect) in [
            ("deny", Decision::Deny),
            ("ask", Decision::Ask),
            ("allow", Decision::Allow),
        ] {
            if let Some(list) = value.get(key) {
                for text in list.as_array().context("permission rules must be arrays")? {
                    self.entries.push(parse(
                        text.as_str().context("permission rule must be a string")?,
                        effect,
                    )?);
                    if self.entries.len() > 1024 {
                        bail!("permission rules exceed 1024 entries")
                    }
                }
            }
        }
        Ok(())
    }
    pub fn decision(
        &self,
        workspace: &Path,
        name: &str,
        input: &Value,
    ) -> Result<Option<Decision>> {
        if ["bash", "powershell"].contains(&name) {
            let command = util::string(input, "command")?;
            let parsed = shell_commands(name, command);
            let has_denies = self.entries.iter().any(|rule| {
                rule.effect == Decision::Deny
                    && (rule.tool.is_match(name)
                        || name == "powershell" && rule.tool.is_match("bash"))
            });
            if has_denies
                && parsed.parts.iter().any(|part| {
                    let words = command_words(part);
                    words.first().is_some_and(|word| {
                        [
                            "env",
                            "command",
                            "sudo",
                            "bash",
                            "sh",
                            "zsh",
                            "pwsh",
                            "powershell",
                            "cmd",
                            "eval",
                            "exec",
                        ]
                        .contains(&word.to_ascii_lowercase().as_str())
                    }) || words.iter().any(|word| {
                        ["-e", "-c", "-command", "-encodedcommand", "-file"]
                            .contains(&word.to_ascii_lowercase().as_str())
                    })
                })
            {
                return Ok(Some(Decision::Deny));
            }
            let mut best = None;
            let mut all_allowed = !parsed.parts.is_empty();
            for part in &parsed.parts {
                let decision = self.one(name, Some(part), false)?;
                if decision == Some(Decision::Deny) {
                    return Ok(decision);
                }
                if decision == Some(Decision::Ask) {
                    best = decision
                }
                all_allowed &= decision == Some(Decision::Allow);
            }
            if !parsed.safe {
                // Never let a broad tool grant or bypass infer authority for
                // a shell structure the prefix parser cannot understand.
                let denied = self.entries.iter().any(|rule| {
                    rule.effect == Decision::Deny
                        && (rule.tool.is_match(name)
                            || name == "powershell" && rule.tool.is_match("bash"))
                });
                return Ok(Some(if denied {
                    Decision::Deny
                } else {
                    Decision::Ask
                }));
            }
            Ok(if best.is_some() {
                best
            } else if all_allowed {
                Some(Decision::Allow)
            } else {
                None
            })
        } else if ["read", "write", "edit", "grep", "ls"].contains(&name) {
            let decision =
                self.path_decision(workspace, name, input["path"].as_str().unwrap_or("."))?;
            if decision != Some(Decision::Deny)
                && ["grep", "ls"].contains(&name)
                && self.entries.iter().any(|rule| {
                    rule.effect == Decision::Ask
                        && (rule.tool.is_match("read") || rule.tool.is_match(name))
                })
            {
                Ok(Some(Decision::Ask))
            } else {
                Ok(decision)
            }
        } else {
            self.one(name, None, false)
        }
    }
    pub fn path_decision(
        &self,
        workspace: &Path,
        name: &str,
        raw: &str,
    ) -> Result<Option<Decision>> {
        let path = util::in_workspace(workspace, raw)?;
        let relative = path
            .strip_prefix(util::canonical(workspace)?)?
            .to_str()
            .context("non-UTF-8 permission path")?
            .replace(std::path::MAIN_SEPARATOR, "/");
        self.one(
            name,
            Some(if relative.is_empty() { "." } else { &relative }),
            true,
        )
    }
    fn one(&self, name: &str, value: Option<&str>, file: bool) -> Result<Option<Decision>> {
        let mut result = None;
        for rule in &self.entries {
            let tool = rule.tool.is_match(name)
                || name == "powershell" && rule.tool.is_match("bash")
                || file && ["grep", "ls"].contains(&name) && rule.tool.is_match("read")
                || file
                    && ["write", "edit"].contains(&name)
                    && rule.effect != Decision::Allow
                    && rule.tool.is_match("read")
                || file && name == "write" && rule.tool.is_match("edit");
            if !tool {
                continue;
            }
            let matched = match (&rule.scope, value) {
                (None, _) => true,
                (Some(pattern), Some(value)) if file => path_glob(pattern)?.is_match(value),
                (Some(pattern), Some(value)) => {
                    let pattern = commands(pattern.strip_suffix(":*").unwrap_or(pattern));
                    if !pattern.safe || pattern.parts.len() != 1 {
                        false
                    } else {
                        let mut actual = comparable_words(value, name);
                        let expected = comparable_words(&pattern.parts[0], name);
                        if rule.effect == Decision::Deny
                            && expected
                                .first()
                                .is_some_and(|program| !program.contains(['/', '\\']))
                            && let Some(program) = actual.first_mut()
                        {
                            *program = program
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or(program)
                                .to_string();
                            if cfg!(windows)
                                && let Some(base) = program.strip_suffix(".exe")
                            {
                                *program = base.to_string();
                            }
                        }
                        if rule.scope.as_ref().unwrap().ends_with(":*") {
                            actual.starts_with(&expected)
                        } else {
                            actual == expected
                        }
                    }
                }
                _ => false,
            };
            if !matched {
                continue;
            }
            match rule.effect {
                Decision::Deny => return Ok(Some(Decision::Deny)),
                Decision::Ask => result = Some(Decision::Ask),
                Decision::Allow if result.is_none() => result = Some(Decision::Allow),
                _ => {}
            }
        }
        Ok(result)
    }
    pub fn grant_rule(workspace: &Path, name: &str, input: &Value) -> Result<Option<String>> {
        if ["workspace_trust", "review_delegate", "dangerous_command"].contains(&name) {
            return Ok(None);
        }
        if ["bash", "powershell"].contains(&name) {
            let parsed = shell_commands(name, util::string(input, "command")?);
            if !parsed.safe || parsed.parts.len() != 1 {
                return Ok(None);
            }
            let words = command_words(&parsed.parts[0]);
            if words.iter().any(|word| {
                word.is_empty()
                    || !word
                        .chars()
                        .all(|ch| ch.is_alphanumeric() || "._/:=@+-".contains(ch))
            }) {
                return Ok(None);
            }
            // Remember the executable and subcommand (or first option), never
            // an executable-wide interpreter grant such as bash(node:*).
            if words.len() < 2 {
                return Ok(Some(format!("{name}({})", words[0])));
            }
            return Ok(Some(format!("{name}({}:*)", words[..2].join(" "))));
        }
        if ["read", "write", "edit", "grep", "ls"].contains(&name) {
            let path = util::in_workspace(workspace, input["path"].as_str().unwrap_or("."))?;
            let relative = path
                .strip_prefix(util::canonical(workspace)?)?
                .to_str()
                .context("non-UTF-8 permission path")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if relative.contains(['(', ')', '*', '?', '[', ']', '{', '}']) {
                return Ok(None);
            }
            return Ok(Some(format!(
                "{name}({})",
                if relative.is_empty() { "." } else { &relative }
            )));
        }
        if name.starts_with("mcp__") {
            return Ok(Some(name.into()));
        }
        Ok(None)
    }
}
fn parse(text: &str, effect: Decision) -> Result<Rule> {
    if text.len() > 4096 || text.is_empty() {
        bail!("invalid permission rule length")
    }
    let (tool, scope) = if let Some((tool, scope)) = text.split_once('(') {
        let scope = scope
            .strip_suffix(')')
            .context("permission rule needs a closing parenthesis")?;
        if scope.is_empty() {
            bail!("empty permission scope")
        }
        (tool, Some(scope.to_string()))
    } else {
        (text, None)
    };
    if tool.is_empty()
        || !tool
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_*?-".contains(&byte))
    {
        bail!("invalid permission tool pattern")
    }
    if let Some(scope) = &scope
        && ["read", "write", "edit", "grep", "ls"].contains(&tool)
    {
        path_glob(scope)?;
    }
    Ok(Rule {
        effect,
        tool: GlobBuilder::new(tool).build()?.compile_matcher(),
        scope,
    })
}
fn path_glob(pattern: &str) -> Result<GlobMatcher> {
    let pattern = pattern.replace('\\', "/");
    if Path::new(&pattern).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("permission paths must be workspace relative without ..")
    }
    let pattern = pattern.trim_start_matches("./");
    Ok(
        GlobBuilder::new(if pattern.is_empty() { "." } else { pattern })
            .literal_separator(true)
            .case_insensitive(cfg!(windows))
            .build()?
            .compile_matcher(),
    )
}

pub struct Commands {
    pub parts: Vec<String>,
    pub safe: bool,
}
fn shell_commands(name: &str, text: &str) -> Commands {
    let mut parsed = commands(text);
    if name == "powershell"
        && (text.contains("''")
            || text.contains("\"\"")
            || text.contains("--%")
            || text.contains('`'))
    {
        parsed.safe = false;
    }
    parsed
}
pub fn commands(text: &str) -> Commands {
    let mut parts = vec![];
    let mut part = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut safe = true;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if escaped {
            part.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            part.push(ch);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            part.push(ch);
            if ch == q {
                quote = None
            } else if q == '"' && (ch == '$' || ch == '`') {
                safe = false
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            part.push(ch);
            continue;
        }
        if [';', '&', '|', '\n', '\r'].contains(&ch) {
            if ch == '&' || ch == '|' {
                if chars.peek() == Some(&ch) {
                    chars.next();
                } else if ch == '&' {
                    safe = false
                }
            }
            if !part.trim().is_empty() {
                parts.push(normalize_command(&part));
                part.clear()
            }
        } else {
            if ['$', '`', '(', ')', '{', '}', '<', '>'].contains(&ch) {
                safe = false
            }
            part.push(ch)
        }
    }
    if quote.is_some() || escaped {
        safe = false
    }
    if !part.trim().is_empty() {
        parts.push(normalize_command(&part))
    }
    Commands { parts, safe }
}
fn normalize_command(text: &str) -> String {
    text.trim().to_string()
}
pub(crate) fn command_words(text: &str) -> Vec<String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut started = false;
    for ch in text.chars() {
        if escape {
            word.push(ch);
            escape = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escape = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None
            } else {
                word.push(ch)
            }
            started = true;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            started = true;
            continue;
        }
        if ch.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false
            }
        } else {
            word.push(ch);
            started = true
        }
    }
    if started {
        words.push(word)
    }
    words
}
fn comparable_words(text: &str, name: &str) -> Vec<String> {
    let mut words = command_words(text);
    if name == "powershell" {
        for word in &mut words {
            *word = word.to_ascii_lowercase();
        }
    } else if cfg!(windows)
        && let Some(program) = words.first_mut()
    {
        *program = program.to_ascii_lowercase();
    }
    words
}

pub fn persist_project(workspace: &Path, rule: &str) -> Result<()> {
    parse(rule, Decision::Allow)?;
    let directory = workspace.join(".minicode");
    if util::is_link(&directory) {
        bail!("refusing linked project settings directory")
    }
    fs::create_dir_all(&directory)?;
    crate::settings_file::update(&directory.join("settings.local.json"), |value| {
        if value.is_null() {
            *value = json!({})
        }
        if value["permissions"].is_null() {
            value["permissions"] = json!({})
        }
        if value["permissions"]["allow"].is_null() {
            value["permissions"]["allow"] = json!([])
        }
        let allow = value["permissions"]["allow"]
            .as_array_mut()
            .context("invalid project permission allow list")?;
        if !allow.iter().any(|item| item == rule) {
            allow.push(json!(rule))
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compound_segments_and_conservative_syntax_preserve_deny_priority() {
        let mut rules = Rules::default();
        rules
            .extend(&json!({"allow":["bash(npm test:*)"],"deny":["bash(rm -rf:*)"]}))
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            rules
                .decision(
                    root.path(),
                    "bash",
                    &json!({"command":"npm test -- --watch"})
                )
                .unwrap(),
            Some(Decision::Allow)
        );
        for operator in [";", "&&", "||", "|"] {
            assert_eq!(
                rules
                    .decision(
                        root.path(),
                        "bash",
                        &json!({"command":format!("npm test {operator} rm -rf x")})
                    )
                    .unwrap(),
                Some(Decision::Deny)
            );
        }
        assert_eq!(
            rules
                .decision(
                    root.path(),
                    "bash",
                    &json!({"command":"npm test; 'rm' '-rf' x"})
                )
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(root.path(), "bash", &json!({"command":"/bin/rm -rf x"}))
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            Rules::grant_rule(
                root.path(),
                "bash",
                &json!({"command":"node 'safe command'"})
            )
            .unwrap(),
            None
        );
        assert_eq!(
            rules
                .decision(root.path(), "bash", &json!({"command":"env rm -rf x"}))
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(
                    root.path(),
                    "bash",
                    &json!({"command":"bash -c 'rm -rf x'"})
                )
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(
                    root.path(),
                    "bash",
                    &json!({"command":"npm test $(touch x)"})
                )
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(root.path(), "bash", &json!({"command":"npm testing"}))
                .unwrap(),
            None
        );
    }
    #[test]
    fn asking_and_path_rules_cannot_be_overridden_by_broader_allows() {
        let mut rules = Rules::default();
        rules.extend(&json!({"deny":["read(.env)","read(**/*.pem)"],"ask":["bash(git push:*)"],"allow":["read(**)","bash"]})).unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join(".env"), "secret").unwrap();
        assert_eq!(
            rules
                .path_decision(root.path(), "grep", "sub/../.env")
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules.path_decision(root.path(), "read", "key.pem").unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules.path_decision(root.path(), "edit", ".env").unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(root.path(), "bash", &json!({"command":"git push --force"}))
                .unwrap(),
            Some(Decision::Ask)
        );
    }

    #[test]
    fn user_project_local_and_cli_rules_union_without_erasing_denies() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let workspace = dir.path().join("repo");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(workspace.join(".minicode")).unwrap();
        fs::write(
            user.join("settings.json"),
            json!({"permissions":{"deny":["read(.env)"]}}).to_string(),
        )
        .unwrap();
        fs::write(
            workspace.join(".minicode/settings.json"),
            json!({"permissions":{"allow":["read(**)"],"ask":["bash(git push:*)"]}}).to_string(),
        )
        .unwrap();
        fs::write(
            workspace.join(".minicode/settings.local.json"),
            json!({"permissions":{"allow":["bash(git push:*)"]}}).to_string(),
        )
        .unwrap();
        let rules = Rules::load(
            &workspace,
            &user,
            &json!({"allow":["read(.env)","bash"],"deny":["write(private.txt)"]}),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            rules.path_decision(&workspace, "read", ".env").unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(
                    &workspace,
                    "bash",
                    &json!({"command":"git push origin main"})
                )
                .unwrap(),
            Some(Decision::Ask)
        );
        assert_eq!(
            rules
                .path_decision(&workspace, "write", "private.txt")
                .unwrap(),
            Some(Decision::Deny)
        );
    }
}
