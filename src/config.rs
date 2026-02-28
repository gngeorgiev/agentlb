use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runner {
    pub default_command: String,
    pub default_command_args: Vec<String>,
    pub login_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sessions {
    pub alias_pattern: String,
    pub assignment_history_window: usize,
    pub pick_behavior: String,
    pub stale_sec: i64,
    pub busy_penalty: i64,
    pub unknown_usage_left_percent: i64,
    pub usage_primary_weight_percent: i64,
    pub usage_secondary_weight_percent: i64,
    pub restart_penalty_per_restart: i64,
    pub restart_penalty_cap: i64,
    pub staleness_penalty_max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub runner: Runner,
    pub sessions: Sessions,
}

#[derive(Debug, Default, Deserialize)]
struct PartialRunner {
    default_command: Option<String>,
    default_command_args: Option<Vec<String>>,
    login_command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialSessions {
    alias_pattern: Option<String>,
    assignment_history_window: Option<usize>,
    pick_behavior: Option<String>,
    stale_sec: Option<i64>,
    busy_penalty: Option<i64>,
    unknown_usage_left_percent: Option<i64>,
    usage_primary_weight_percent: Option<i64>,
    usage_secondary_weight_percent: Option<i64>,
    restart_penalty_per_restart: Option<i64>,
    restart_penalty_cap: Option<i64>,
    staleness_penalty_max: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    runner: Option<PartialRunner>,
    sessions: Option<PartialSessions>,
}

pub fn default_config() -> Config {
    Config {
        runner: Runner {
            default_command: "codex".to_string(),
            default_command_args: vec![],
            login_command: "codex login".to_string(),
        },
        sessions: Sessions {
            alias_pattern: "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$".to_string(),
            assignment_history_window: 30,
            pick_behavior: "round_robin".to_string(),
            stale_sec: 420,
            busy_penalty: 5,
            unknown_usage_left_percent: 30,
            usage_primary_weight_percent: 60,
            usage_secondary_weight_percent: 40,
            restart_penalty_per_restart: 2,
            restart_penalty_cap: 20,
            staleness_penalty_max: 10,
        },
    }
}

pub fn agentlb_root() -> io::Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join(".agentlb"))
}

pub fn config_path() -> io::Result<PathBuf> {
    Ok(agentlb_root()?.join("config.toml"))
}

pub fn load(path: &Path) -> io::Result<Config> {
    let defaults = default_config();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(defaults),
        Err(err) => return Err(err),
    };

    let content = std::str::from_utf8(&bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("config is not valid UTF-8 ({}): {}", path.display(), err),
        )
    })?;
    let partial: PartialConfig = toml::from_str(content).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse config {}: {}", path.display(), err),
        )
    })?;

    let mut cfg = defaults.clone();
    if let Some(r) = partial.runner {
        if let Some(v) = r.default_command {
            cfg.runner.default_command = v;
        }
        if let Some(v) = r.default_command_args {
            cfg.runner.default_command_args = v;
        }
        if let Some(v) = r.login_command {
            cfg.runner.login_command = v;
        }
    }
    if let Some(s) = partial.sessions {
        if let Some(v) = s.alias_pattern {
            cfg.sessions.alias_pattern = v;
        }
        if let Some(v) = s.assignment_history_window {
            cfg.sessions.assignment_history_window = v;
        }
        if let Some(v) = s.pick_behavior {
            cfg.sessions.pick_behavior = v;
        }
        if let Some(v) = s.stale_sec {
            cfg.sessions.stale_sec = v;
        }
        if let Some(v) = s.busy_penalty {
            cfg.sessions.busy_penalty = v;
        }
        if let Some(v) = s.unknown_usage_left_percent {
            cfg.sessions.unknown_usage_left_percent = v;
        }
        if let Some(v) = s.usage_primary_weight_percent {
            cfg.sessions.usage_primary_weight_percent = v;
        }
        if let Some(v) = s.usage_secondary_weight_percent {
            cfg.sessions.usage_secondary_weight_percent = v;
        }
        if let Some(v) = s.restart_penalty_per_restart {
            cfg.sessions.restart_penalty_per_restart = v;
        }
        if let Some(v) = s.restart_penalty_cap {
            cfg.sessions.restart_penalty_cap = v;
        }
        if let Some(v) = s.staleness_penalty_max {
            cfg.sessions.staleness_penalty_max = v;
        }
    }

    if cfg.runner.default_command.trim().is_empty() {
        cfg.runner.default_command = defaults.runner.default_command;
    }
    if cfg.runner.login_command.trim().is_empty() {
        cfg.runner.login_command = defaults.runner.login_command;
    }
    if cfg.sessions.alias_pattern.trim().is_empty() {
        cfg.sessions.alias_pattern = defaults.sessions.alias_pattern;
    }
    if cfg.sessions.assignment_history_window == 0 {
        cfg.sessions.assignment_history_window = defaults.sessions.assignment_history_window;
    }
    if cfg.sessions.pick_behavior != "round_robin" && cfg.sessions.pick_behavior != "last" {
        cfg.sessions.pick_behavior = defaults.sessions.pick_behavior;
    }
    if cfg.sessions.stale_sec <= 0 {
        cfg.sessions.stale_sec = defaults.sessions.stale_sec;
    }
    if cfg.sessions.busy_penalty < 0 {
        cfg.sessions.busy_penalty = defaults.sessions.busy_penalty;
    }
    if !(0..=100).contains(&cfg.sessions.unknown_usage_left_percent) {
        cfg.sessions.unknown_usage_left_percent = defaults.sessions.unknown_usage_left_percent;
    }
    if cfg.sessions.usage_primary_weight_percent < 0 {
        cfg.sessions.usage_primary_weight_percent = defaults.sessions.usage_primary_weight_percent;
    }
    if cfg.sessions.usage_secondary_weight_percent < 0 {
        cfg.sessions.usage_secondary_weight_percent =
            defaults.sessions.usage_secondary_weight_percent;
    }
    if cfg.sessions.usage_primary_weight_percent + cfg.sessions.usage_secondary_weight_percent <= 0
    {
        cfg.sessions.usage_primary_weight_percent = defaults.sessions.usage_primary_weight_percent;
        cfg.sessions.usage_secondary_weight_percent =
            defaults.sessions.usage_secondary_weight_percent;
    }
    if cfg.sessions.restart_penalty_per_restart < 0 {
        cfg.sessions.restart_penalty_per_restart = defaults.sessions.restart_penalty_per_restart;
    }
    if cfg.sessions.restart_penalty_cap < 0 {
        cfg.sessions.restart_penalty_cap = defaults.sessions.restart_penalty_cap;
    }
    if cfg.sessions.staleness_penalty_max < 0 {
        cfg.sessions.staleness_penalty_max = defaults.sessions.staleness_penalty_max;
    }

    Ok(cfg)
}

pub fn ensure_default_config_file(path: &Path, cfg: &Config) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    write_config_file(path, cfg)
}

pub fn write_config_file(path: &Path, cfg: &Config) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid config path"))?;
    fs::create_dir_all(parent)?;
    crate::state::set_mode(parent, 0o700)?;
    let content = toml::to_string(cfg)
        .map_err(|err| io::Error::other(format!("serialize config failed: {}", err)))?;
    fs::write(path, content)?;
    crate::state::set_mode(path, 0o600)
}

pub fn split_command(cmd: &str) -> io::Result<(String, Vec<String>)> {
    let fields: Vec<String> = cmd
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();
    if fields.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
    }
    Ok((fields[0].clone(), fields[1..].to_vec()))
}
