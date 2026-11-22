use crate::util;
use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path},
};

const FILE_TOOLS: &[&str] = &["read", "write", "edit", "grep", "ls", "glob"];
const READ_DISCOVERY_TOOLS: &[&str] = &["grep", "ls", "glob"];

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
            rule.effect == Decision::Deny && FILE_TOOLS.iter().any(|name| rule.tool.is_match(name))
        })
    }
    pub fn matched_file_deny_in_command(&self, workspace: &Path, command: &str) -> Option<String> {
        let deny_matchers: Vec<GlobMatcher> = self
            .entries
            .iter()
            .filter(|rule| {
                rule.effect == Decision::Deny
                    && FILE_TOOLS.iter().any(|name| rule.tool.is_match(name))
            })
            .filter_map(|rule| rule.scope.as_deref())
            .filter_map(|scope| path_glob(scope).ok())
            .collect();
        if deny_matchers.is_empty() {
            return None;
        }
        let canonical_workspace = util::canonical(workspace).ok()?;
        let mut candidates = Vec::new();
        for part in commands(command).parts {
            for word in command_words(&part) {
                collect_path_candidates(&word, &mut candidates);
            }
            for word in raw_quoted_words(&part) {
                collect_path_candidates(&word, &mut candidates);
            }
        }
        let mut seen = std::collections::HashSet::new();
        for candidate in candidates {
            if !seen.insert(candidate.clone()) {
                continue;
            }
            let Some(relative) = normalize_candidate_in_workspace(&canonical_workspace, &candidate)
            else {
                continue;
            };
            if deny_matchers
                .iter()
                .any(|matcher| matcher.is_match(&relative))
            {
                return Some(relative);
            }
        }
        None
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
        } else if FILE_TOOLS.contains(&name) {
            let decision =
                self.path_decision(workspace, name, input["path"].as_str().unwrap_or("."))?;
            if decision != Some(Decision::Deny)
                && READ_DISCOVERY_TOOLS.contains(&name)
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
                || file && READ_DISCOVERY_TOOLS.contains(&name) && rule.tool.is_match("read")
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
        if FILE_TOOLS.contains(&name) {
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
        && FILE_TOOLS.contains(&tool)
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
fn raw_quoted_words(text: &str) -> Vec<String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for ch in text.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
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
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    if started {
        words.push(word);
    }
    words
}
fn strip_token_wrappers(token: &str) -> &str {
    let mut s = token.trim();
    loop {
        let prev = s;
        s = s.trim_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
        });
        for prefix in [
            "2>>", "1>>", "&>>", "0<", "1>", "2>", "&>", "<<", ">>", "<", ">", "@",
        ] {
            if let Some(rest) = s.strip_prefix(prefix) {
                s = rest;
                break;
            }
        }
        if s == prev {
            break;
        }
    }
    s
}
fn push_trimmed_and_kv(token: &str, out: &mut Vec<String>) {
    let trimmed = token.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    let stripped = strip_token_wrappers(trimmed);
    if !stripped.is_empty() && stripped != trimmed {
        out.push(stripped.to_string());
    }
    for source in [trimmed, stripped] {
        if let Some((_, rhs)) = source.split_once('=') {
            let rhs_stripped = strip_token_wrappers(rhs);
            if !rhs_stripped.is_empty() {
                out.push(rhs_stripped.to_string());
            }
        }
        if source.starts_with('-')
            && let Some((_, rhs)) = source.split_once(':')
        {
            let rhs_stripped = strip_token_wrappers(rhs);
            if !rhs_stripped.is_empty() {
                out.push(rhs_stripped.to_string());
            }
        }
    }
}
fn collect_path_candidates(token: &str, out: &mut Vec<String>) {
    push_trimmed_and_kv(token, out);
    let is_delimiter = |ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '\'' | '"'
                    | '`'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | ';'
                    | '<'
                    | '>'
                    | '|'
                    | '&'
            )
    };
    if token.contains(is_delimiter) {
        for piece in token.split(is_delimiter) {
            if !piece.is_empty() {
                push_trimmed_and_kv(piece, out);
            }
        }
    }
}
fn normalize_candidate_in_workspace(canonical_workspace: &Path, candidate: &str) -> Option<String> {
    if candidate.is_empty() || candidate.contains('\0') || candidate.contains("://") {
        return None;
    }
    let normalized = if Path::new(candidate).is_absolute() {
        std::borrow::Cow::Borrowed(candidate)
    } else {
        std::borrow::Cow::Owned(candidate.replace('\\', "/"))
    };
    let raw_path = Path::new(normalized.as_ref());
    let target = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        canonical_workspace.join(raw_path)
    };
    if let Ok(resolved) = util::canonical(&target) {
        if !resolved.starts_with(canonical_workspace) {
            return None;
        }
        let relative = resolved
            .strip_prefix(canonical_workspace)
            .ok()?
            .to_str()?
            .replace(std::path::MAIN_SEPARATOR, "/");
        return Some(if relative.is_empty() {
            ".".into()
        } else {
            relative
        });
    }
    if !candidate.contains(['*', '?', '[', ']', '{', '}']) {
        return None;
    }
    lexical_in_workspace(canonical_workspace, raw_path, candidate)
}
fn normalize_abs_components(path: &Path) -> Option<String> {
    let mut prefix_parts = Vec::new();
    let mut normal_parts = Vec::new();
    let mut has_root = false;
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => {
                prefix_parts.push(p.as_os_str().to_string_lossy().replace('\\', "/"));
            }
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                normal_parts.pop()?;
            }
            Component::Normal(part) => normal_parts.push(part.to_string_lossy().into_owned()),
        }
    }
    let mut out = prefix_parts.join("");
    if has_root {
        out.push('/');
    }
    out.push_str(&normal_parts.join("/"));
    Some(out)
}
fn lexical_in_workspace(
    canonical_workspace: &Path,
    raw_path: &Path,
    candidate: &str,
) -> Option<String> {
    let rel_slice = if raw_path.is_absolute() {
        let ws_raw = canonical_workspace.to_string_lossy().replace('\\', "/");
        let ws = if ws_raw == "/" {
            "/"
        } else {
            ws_raw.trim_end_matches('/')
        };
        let normalized_abs = normalize_abs_components(raw_path)?;
        let trimmed = if normalized_abs == "/" {
            "/"
        } else {
            normalized_abs.trim_end_matches('/')
        };
        let suffix = if ws == "/" {
            normalized_abs.trim_start_matches('/')
        } else {
            let eq = if cfg!(windows) {
                trimmed.eq_ignore_ascii_case(ws)
            } else {
                trimmed == ws
            };
            if eq {
                ""
            } else {
                let starts = if cfg!(windows) {
                    normalized_abs
                        .get(..ws.len())
                        .is_some_and(|p| p.eq_ignore_ascii_case(ws))
                        && normalized_abs.as_bytes().get(ws.len()) == Some(&b'/')
                } else {
                    normalized_abs.starts_with(ws)
                        && normalized_abs.as_bytes().get(ws.len()) == Some(&b'/')
                };
                if !starts {
                    return None;
                }
                normalized_abs
                    .get(ws.len()..)
                    .unwrap_or("")
                    .trim_start_matches('/')
            }
        };
        suffix.to_string()
    } else {
        if raw_path
            .components()
            .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
        {
            return None;
        }
        candidate.replace('\\', "/")
    };
    let mut parts = Vec::new();
    for segment in rel_slice.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let mut prefix = canonical_workspace.to_path_buf();
    for segment in &parts {
        if segment.contains(['*', '?', '[', ']', '{', '}']) {
            break;
        }
        prefix.push(segment);
    }
    if let Ok(resolved_prefix) = util::canonical(&prefix)
        && !resolved_prefix.starts_with(canonical_workspace)
    {
        return None;
    }
    Some(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
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

    #[test]
    fn glob_tool_shares_file_permission_rules() {
        let mut rules = Rules::default();
        rules
            .extend(&json!({
                "deny": ["read(.env)", "glob(secrets/**)"],
                "ask": ["read(keys/*.pem)"],
                "allow": ["read(**)"]
            }))
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        assert!(rules.has_file_denies());
        assert_eq!(
            rules
                .decision(root.path(), "glob", &json!({"path": "sub/../.env"}))
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(root.path(), "glob", &json!({"path": "secrets/token"}))
                .unwrap(),
            Some(Decision::Deny)
        );
        assert_eq!(
            rules
                .decision(root.path(), "glob", &json!({"path": "src"}))
                .unwrap(),
            Some(Decision::Ask)
        );

        let mut allow_only = Rules::default();
        allow_only.extend(&json!({"allow": ["read(**)"]})).unwrap();
        assert_eq!(
            allow_only
                .decision(root.path(), "glob", &json!({"path": "src"}))
                .unwrap(),
            Some(Decision::Allow)
        );
        assert_eq!(
            Rules::grant_rule(root.path(), "glob", &json!({"path": "src"})).unwrap(),
            Some("glob(src)".into())
        );
        assert!(
            allow_only
                .extend(&json!({"deny": ["glob(../outside)"]}))
                .is_err()
        );

        let mut glob_deny_only = Rules::default();
        glob_deny_only
            .extend(&json!({"deny": ["glob(private/**)"]}))
            .unwrap();
        assert!(glob_deny_only.has_file_denies());
    }

    #[test]
    fn matched_file_deny_in_command_detects_sensitive_references() {
        let root = tempfile::tempdir().unwrap();
        let workspace = util::canonical(root.path()).unwrap();
        fs::create_dir(workspace.join("sub")).unwrap();
        fs::create_dir(workspace.join("secrets")).unwrap();
        fs::create_dir(workspace.join("keys")).unwrap();
        fs::create_dir(workspace.join("dir with space")).unwrap();
        fs::write(workspace.join(".env"), "SECRET=1").unwrap();
        fs::write(workspace.join("keys/prod.pem"), "KEY").unwrap();
        fs::write(workspace.join("dir with space/secret.pem"), "KEY").unwrap();

        let mut no_file_denies = Rules::default();
        no_file_denies
            .extend(&json!({"deny": ["bash(rm -rf:*)"]}))
            .unwrap();
        assert_eq!(
            no_file_denies.matched_file_deny_in_command(&workspace, "cat .env"),
            None
        );

        let mut rules = Rules::default();
        rules
            .extend(&json!({
                "deny": [
                    "read(.env)",
                    "read(**/*.pem)",
                    "write(secrets/**)",
                    "glob(private/**)"
                ],
                "allow": ["read(.env)", "bash"]
            }))
            .unwrap();

        for (command, expected) in [
            ("cat .env", ".env"),
            ("echo ok && cat sub/../.env", ".env"),
            ("ls; cat './.env' | wc -l", ".env"),
            ("cat <.env", ".env"),
            ("grep --file=.env SECRET", ".env"),
            ("echo $(cat .env)", ".env"),
            ("echo `cat .env`", ".env"),
            ("node -e \"fs.readFileSync('.env')\"", ".env"),
            ("[IO.File]::ReadAllText('sub/../.env')", ".env"),
            ("Get-Content -Path:.env", ".env"),
            ("Get-Content sub\\..\\.env", ".env"),
            ("echo hi > secrets/token.txt", "secrets/token.txt"),
            ("cat keys/prod.pem", "keys/prod.pem"),
            ("cat secrets/*", "secrets/*"),
            ("ls private/data.txt", "private/data.txt"),
            (
                "cat \"dir with space/secret.pem\"",
                "dir with space/secret.pem",
            ),
        ] {
            assert_eq!(
                rules.matched_file_deny_in_command(&workspace, command),
                Some(expected.to_string()),
                "command: {command}"
            );
        }

        let abs_env = workspace.join(".env");
        assert_eq!(
            rules.matched_file_deny_in_command(
                &workspace,
                &format!("cat \"{}\"", abs_env.display())
            ),
            Some(".env".into())
        );

        let abs_wildcard = workspace.join("secrets").join("*");
        assert_eq!(
            rules.matched_file_deny_in_command(
                &workspace,
                &format!("cat \"{}\"", abs_wildcard.display())
            ),
            Some("secrets/*".into())
        );

        for safe in [
            "git status",
            "cargo test --locked",
            "cat README.md",
            "cat .env.example",
            "cat ../outside/.env",
        ] {
            assert_eq!(
                rules.matched_file_deny_in_command(&workspace, safe),
                None,
                "safe command unexpectedly matched: {safe}"
            );
        }
    }
}
