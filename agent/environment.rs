use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

fn command_line(program: &str, args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .chars()
            .take(4096)
            .collect()
    })
}
pub fn git_summary(workspace: &Path) -> (String, bool) {
    type Entries = HashMap<PathBuf, (Instant, (String, bool))>;
    static CACHE: OnceLock<Mutex<Entries>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, value)) = cache.lock().unwrap().get(workspace)
        && at.elapsed() < Duration::from_secs(2)
    {
        return value.clone();
    }
    let value = command_line(
        "git",
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--branch",
            "--untracked-files=normal",
        ],
        Some(workspace),
    )
    .map(|output| {
        let mut lines = output.lines();
        let branch = lines
            .next()
            .unwrap_or_default()
            .trim_start_matches("## ")
            .trim_start_matches("No commits yet on ")
            .trim_start_matches("Initial commit on ")
            .split("...")
            .next()
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        (branch, lines.next().is_some())
    })
    .unwrap_or_else(|| ("unavailable".into(), false));
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 32 {
        cache.clear()
    }
    cache.insert(workspace.into(), (Instant::now(), value.clone()));
    value
}
pub fn git_branch(workspace: &Path) -> Option<String> {
    let (branch, _) = git_summary(workspace);
    (branch != "unavailable" && !branch.is_empty()).then_some(branch)
}
pub fn prompt(workspace: &Path) -> String {
    static OS: OnceLock<String> = OnceLock::new();
    static SHELL: OnceLock<String> = OnceLock::new();
    let os = OS.get_or_init(|| {
        #[cfg(windows)]
        let version = command_line("cmd", &["/c", "ver"], None);
        #[cfg(not(windows))]
        let version = command_line("uname", &["-sr"], None);
        format!(
            "{} {}",
            std::env::consts::OS,
            version.unwrap_or_else(|| "version unavailable".into())
        )
    });
    let shell = SHELL.get_or_init(|| {
        #[cfg(windows)]
        {
            crate::tools::windows_shell_guidance()
        }
        #[cfg(not(windows))]
        {
            command_line("bash", &["--version"], None)
                .and_then(|text| text.lines().next().map(str::to_string))
                .unwrap_or_else(|| "bash (version unavailable)".into())
        }
    });
    let (branch, dirty) = git_summary(workspace);
    format!(
        "\n环境信息（数据）：\n日期：{}\nOS：{}\nShell：{}\n工作目录：{}\nGit：{}；未提交改动：{}\n",
        chrono::Local::now().format("%Y-%m-%d"),
        os,
        shell,
        workspace.display(),
        branch,
        if dirty { "有" } else { "无或不可用" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a3_environment_counts_untracked_work_without_disclosing_names() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(command_line("git", &["init", "-q"], Some(workspace.path())).is_some());
        let private_value = "environment-fixture-private-value";
        std::fs::write(workspace.path().join(".env"), private_value).unwrap();
        let text = prompt(workspace.path());
        assert!(text.contains("未提交改动：有"), "{text}");
        assert!(!text.contains(".env"));
        assert!(!text.contains(private_value));
    }
}
