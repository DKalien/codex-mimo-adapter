use anyhow::{anyhow, Context};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const PROJECT_ENV_FILENAME: &str = ".codex-opencode-adapter.env";

#[derive(Debug, Clone)]
pub struct ProjectPaths {
    pub root: PathBuf,
    pub env_file: PathBuf,
    pub agents_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl ProjectPaths {
    pub fn from_current_dir() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let env = current_environment();
        Ok(Self::discover(&cwd, &env))
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self {
            env_file: root.join(PROJECT_ENV_FILENAME),
            agents_dir: root.join(".codex").join("agents"),
            state_dir: root.join(".codex-opencode"),
            root,
        }
    }

    pub fn discover_from(start: &Path) -> Self {
        for candidate in start.ancestors() {
            let paths = Self::from_root(candidate.to_path_buf());
            if paths.env_file.exists() {
                return paths;
            }
        }
        Self::from_root(start.to_path_buf())
    }

    pub fn discover(start: &Path, env: &HashMap<String, String>) -> Self {
        let local = Self::discover_from(start);
        if local.env_file.exists() {
            return local;
        }

        if let Some(thread_root) = discover_root_from_codex_thread(env) {
            let thread_paths = Self::discover_from(&thread_root);
            if thread_paths.env_file.exists() {
                return thread_paths;
            }
        }

        local
    }
}

pub fn read_project_env(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read project config at {}", path.display()))?;
    parse_env_text(&contents)
}

pub fn parse_env_text(contents: &str) -> anyhow::Result<HashMap<String, String>> {
    let mut values = HashMap::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(anyhow!("invalid env line {}: {}", index + 1, raw_line));
        };
        values.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok(values)
}

pub fn current_environment() -> HashMap<String, String> {
    std::env::vars().collect()
}

fn discover_root_from_codex_thread(env: &HashMap<String, String>) -> Option<PathBuf> {
    let codex_dir = codex_home_dir(env)?;
    if let Some(thread_id) = env.get("CODEX_THREAD_ID") {
        if let Some(path) = find_cwd_in_process_manager(&codex_dir, Some(thread_id))
            .or_else(|| find_cwd_in_sessions(&codex_dir.join("sessions"), Some(thread_id)))
        {
            return Some(path);
        }
    }

    find_cwd_in_process_manager(&codex_dir, None)
        .or_else(|| find_cwd_in_sessions(&codex_dir.join("sessions"), None))
}

fn codex_home_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get("USERPROFILE")
        .or_else(|| env.get("HOME"))
        .map(|home| PathBuf::from(home).join(".codex"))
}

fn find_cwd_in_process_manager(codex_dir: &Path, thread_id: Option<&str>) -> Option<PathBuf> {
    let path = codex_dir.join("process_manager").join("chat_processes.json");
    let contents = fs::read_to_string(path).ok()?;
    let items: Vec<Value> = serde_json::from_str(&contents).ok()?;
    items.into_iter()
        .filter_map(|item| {
            let cwd = item.get("cwd")?.as_str()?;
            let candidate = ProjectPaths::discover_from(Path::new(cwd));
            if !candidate.env_file.exists() {
                return None;
            }

            let updated_at = item.get("updatedAtMs").and_then(Value::as_i64).unwrap_or_default();
            let matches_thread = thread_id.is_none_or(|id| {
                item.get("conversationId").and_then(Value::as_str) == Some(id)
            });
            matches_thread.then(|| (updated_at, PathBuf::from(cwd)))
        })
        .max_by_key(|(updated_at, _)| *updated_at)
        .map(|(_, cwd)| cwd)
}

fn find_cwd_in_sessions(sessions_dir: &Path, thread_id: Option<&str>) -> Option<PathBuf> {
    let entries = fs::read_dir(sessions_dir).ok()?;
    let mut best: Option<(String, PathBuf)> = None;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(cwd) = find_cwd_in_sessions(&path, thread_id) {
                return Some(cwd);
            }
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some((timestamp, cwd)) = find_cwd_in_session_file(&path, thread_id) {
            match &best {
                Some((best_timestamp, _)) if best_timestamp >= &timestamp => {}
                _ => best = Some((timestamp, cwd)),
            }
        }
    }
    best.map(|(_, cwd)| cwd)
}

fn find_cwd_in_session_file(path: &Path, thread_id: Option<&str>) -> Option<(String, PathBuf)> {
    let file = fs::File::open(path).ok()?;
    let mut first_line = String::new();
    BufReader::new(file).read_line(&mut first_line).ok()?;
    let record: Value = serde_json::from_str(&first_line).ok()?;
    let payload = record.get("payload")?;
    let timestamp = record.get("timestamp").and_then(Value::as_str)?.to_string();
    let session_id = payload.get("session_id").and_then(Value::as_str);
    let id = payload.get("id").and_then(Value::as_str);
    let cwd = payload.get("cwd").and_then(Value::as_str)?;
    let candidate = ProjectPaths::discover_from(Path::new(cwd));
    if !candidate.env_file.exists() {
        return None;
    }

    let matches_thread = thread_id.is_none_or(|expected| {
        session_id == Some(expected) || id == Some(expected)
    });
    matches_thread.then(|| (timestamp, PathBuf::from(cwd)))
}
