use crate::state::{Store, now_rfc3339};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::time::{Duration, Instant};

const DEFAULT_STALE_SEC: i64 = 420;
const DEFAULT_BUSY_PENALTY: i64 = 5;
const DEFAULT_UNKNOWN_USAGE_LEFT_PERCENT: i64 = 30;
const PRIMARY_WINDOW_WEIGHT: i64 = 60;
const SECONDARY_WINDOW_WEIGHT: i64 = 40;
const DEFAULT_RESTART_PENALTY_PER_RESTART: i64 = 2;
const DEFAULT_RESTART_PENALTY_CAP: i64 = 20;
const DEFAULT_STALENESS_PENALTY_MAX: i64 = 10;

#[derive(Debug, Clone)]
pub struct ScoringConfig {
    pub stale_sec: i64,
    pub busy_penalty: i64,
    pub unknown_usage_left_percent: i64,
    pub usage_primary_weight_percent: i64,
    pub usage_secondary_weight_percent: i64,
    pub restart_penalty_per_restart: i64,
    pub restart_penalty_cap: i64,
    pub staleness_penalty_max: i64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            stale_sec: DEFAULT_STALE_SEC,
            busy_penalty: DEFAULT_BUSY_PENALTY,
            unknown_usage_left_percent: DEFAULT_UNKNOWN_USAGE_LEFT_PERCENT,
            usage_primary_weight_percent: PRIMARY_WINDOW_WEIGHT,
            usage_secondary_weight_percent: SECONDARY_WINDOW_WEIGHT,
            restart_penalty_per_restart: DEFAULT_RESTART_PENALTY_PER_RESTART,
            restart_penalty_cap: DEFAULT_RESTART_PENALTY_CAP,
            staleness_penalty_max: DEFAULT_STALENESS_PENALTY_MAX,
        }
    }
}

impl From<&crate::config::Sessions> for ScoringConfig {
    fn from(s: &crate::config::Sessions) -> Self {
        Self {
            stale_sec: s.stale_sec,
            busy_penalty: s.busy_penalty,
            unknown_usage_left_percent: s.unknown_usage_left_percent,
            usage_primary_weight_percent: s.usage_primary_weight_percent,
            usage_secondary_weight_percent: s.usage_secondary_weight_percent,
            restart_penalty_per_restart: s.restart_penalty_per_restart,
            restart_penalty_cap: s.restart_penalty_cap,
            staleness_penalty_max: s.staleness_penalty_max,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub daemon: DaemonStatus,
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionStatus>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    #[serde(default)]
    pub pid: i32,
    #[serde(default)]
    pub started_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    #[serde(default)]
    pub health: String,
    #[serde(default)]
    pub app_server: AppServerStatus,
    #[serde(default)]
    pub rate_limits: RateLimits,
    #[serde(default)]
    pub usage_left_percent: i64,
    #[serde(default)]
    pub activity: Activity,
    #[serde(default)]
    pub last_rate_limit_update_at: String,
    #[serde(default)]
    pub last_probe_at: String,
    #[serde(default)]
    pub last_probe_result: String,
    #[serde(default)]
    pub last_selected_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppServerStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub pid: i32,
    #[serde(default)]
    pub restart_count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    #[serde(default)]
    pub limit_id: String,
    #[serde(default)]
    pub primary: Option<RateWindow>,
    #[serde(default)]
    pub secondary: Option<RateWindow>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    #[serde(default)]
    pub used_percent: i64,
    #[serde(default)]
    pub window_duration_mins: i64,
    #[serde(default)]
    pub resets_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(default)]
    pub active_turns: i64,
}

fn default_version() -> u32 {
    1
}

fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

fn window_left(window: &Option<RateWindow>) -> Option<i64> {
    window
        .as_ref()
        .map(|w| (100 - w.used_percent).clamp(0, 100))
}

fn compute_usage_left(sess: &SessionStatus, cfg: &ScoringConfig) -> i64 {
    let primary_left = window_left(&sess.rate_limits.primary);
    let secondary_left = window_left(&sess.rate_limits.secondary);

    match (primary_left, secondary_left) {
        (Some(p), Some(s)) => {
            // Blend short-window and weekly-window capacity so both influence picking.
            let total_weight =
                (cfg.usage_primary_weight_percent + cfg.usage_secondary_weight_percent).max(1);
            (p * cfg.usage_primary_weight_percent + s * cfg.usage_secondary_weight_percent)
                / total_weight
        }
        (Some(p), None) => p,
        (None, Some(s)) => s,
        (None, None) => cfg.unknown_usage_left_percent,
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    alias: String,
    score: i64,
    usage_left: i64,
    active_turns: i64,
    last_selected_at: Option<DateTime<Utc>>,
}

fn to_candidate(
    alias: &str,
    sess: &SessionStatus,
    now: DateTime<Utc>,
    cfg: &ScoringConfig,
) -> Option<Candidate> {
    if sess.health != "healthy" {
        return None;
    }

    let last_update = parse_ts(&sess.last_rate_limit_update_at)?;
    let age = now.signed_duration_since(last_update).num_seconds();
    if age > cfg.stale_sec {
        return None;
    }

    let usage_left = compute_usage_left(sess, cfg).clamp(0, 100);
    let active_turns = sess.activity.active_turns.max(0);
    let restart_penalty = (sess.app_server.restart_count.max(0) * cfg.restart_penalty_per_restart)
        .min(cfg.restart_penalty_cap);
    let staleness_penalty = if age > 0 {
        (age * cfg.staleness_penalty_max / cfg.stale_sec).clamp(0, cfg.staleness_penalty_max)
    } else {
        0
    };

    let mut score = usage_left;
    score -= active_turns * cfg.busy_penalty;
    score -= restart_penalty;
    score -= staleness_penalty;

    Some(Candidate {
        alias: alias.to_string(),
        score,
        usage_left,
        active_turns,
        last_selected_at: parse_ts(&sess.last_selected_at),
    })
}

pub fn load_status(st: &Store) -> io::Result<StatusFile> {
    let bytes = fs::read(&st.status_path)?;
    serde_json::from_slice::<StatusFile>(&bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid status json: {}", err),
        )
    })
}

pub fn save_status(st: &Store, status: &StatusFile) -> io::Result<()> {
    st.write_json_atomic(&st.status_path, status)
}

pub fn pick_best_session_with_config(
    status: &StatusFile,
    now: DateTime<Utc>,
    cfg: &ScoringConfig,
) -> Option<String> {
    let mut candidates: Vec<Candidate> = status
        .sessions
        .iter()
        .filter_map(|(alias, sess)| to_candidate(alias, sess, now, cfg))
        .collect();

    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.usage_left.cmp(&a.usage_left))
            .then_with(|| a.active_turns.cmp(&b.active_turns))
            .then_with(|| match (&a.last_selected_at, &b.last_selected_at) {
                (Some(la), Some(lb)) => la.cmp(lb),
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
            .then_with(|| a.alias.cmp(&b.alias))
    });

    candidates.first().map(|c| c.alias.clone())
}

pub fn pick_best_session_with_retry_and_config(
    st: &Store,
    timeout_ms: u64,
    cfg: &ScoringConfig,
) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Ok(status) = load_status(st)
            && let Some(alias) = pick_best_session_with_config(&status, Utc::now(), cfg)
        {
            return Some(alias);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn mark_selected(st: &Store, alias: &str) {
    if let Ok(mut status) = load_status(st)
        && let Some(sess) = status.sessions.get_mut(alias)
    {
        let now = now_rfc3339();
        sess.last_selected_at = now.clone();
        status.updated_at = now;
        let _ = save_status(st, &status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_uses_usage_then_busy_then_lru() {
        let mut status = StatusFile {
            version: 1,
            updated_at: now_rfc3339(),
            ..StatusFile::default()
        };

        status.sessions.insert(
            "a".to_string(),
            SessionStatus {
                health: "healthy".to_string(),
                last_rate_limit_update_at: now_rfc3339(),
                usage_left_percent: 50,
                rate_limits: RateLimits {
                    primary: Some(RateWindow {
                        used_percent: 50,
                        ..RateWindow::default()
                    }),
                    ..RateLimits::default()
                },
                ..SessionStatus::default()
            },
        );
        status.sessions.insert(
            "b".to_string(),
            SessionStatus {
                health: "healthy".to_string(),
                last_rate_limit_update_at: now_rfc3339(),
                usage_left_percent: 70,
                rate_limits: RateLimits {
                    primary: Some(RateWindow {
                        used_percent: 30,
                        ..RateWindow::default()
                    }),
                    ..RateLimits::default()
                },
                ..SessionStatus::default()
            },
        );

        let got = pick_best_session_with_config(&status, Utc::now(), &ScoringConfig::default());
        assert_eq!(got.as_deref(), Some("b"));
    }

    #[test]
    fn pick_rejects_stale_and_unhealthy() {
        let now = Utc::now();
        let stale = (now - chrono::Duration::seconds(DEFAULT_STALE_SEC + 1)).to_rfc3339();

        let mut status = StatusFile::default();
        status.sessions.insert(
            "a".to_string(),
            SessionStatus {
                health: "healthy".to_string(),
                last_rate_limit_update_at: stale,
                ..SessionStatus::default()
            },
        );
        status.sessions.insert(
            "b".to_string(),
            SessionStatus {
                health: "unhealthy".to_string(),
                last_rate_limit_update_at: now_rfc3339(),
                ..SessionStatus::default()
            },
        );

        assert!(pick_best_session_with_config(&status, now, &ScoringConfig::default()).is_none());
    }

    #[test]
    fn pick_balances_primary_and_secondary_windows() {
        let mut status = StatusFile::default();
        let now = now_rfc3339();

        // a: excellent short-window, poor weekly-window.
        status.sessions.insert(
            "a".to_string(),
            SessionStatus {
                health: "healthy".to_string(),
                last_rate_limit_update_at: now.clone(),
                rate_limits: RateLimits {
                    primary: Some(RateWindow {
                        used_percent: 10, // 90 left
                        ..RateWindow::default()
                    }),
                    secondary: Some(RateWindow {
                        used_percent: 95, // 5 left
                        ..RateWindow::default()
                    }),
                    ..RateLimits::default()
                },
                ..SessionStatus::default()
            },
        );

        // b: good short-window, good weekly-window.
        status.sessions.insert(
            "b".to_string(),
            SessionStatus {
                health: "healthy".to_string(),
                last_rate_limit_update_at: now,
                rate_limits: RateLimits {
                    primary: Some(RateWindow {
                        used_percent: 35, // 65 left
                        ..RateWindow::default()
                    }),
                    secondary: Some(RateWindow {
                        used_percent: 40, // 60 left
                        ..RateWindow::default()
                    }),
                    ..RateLimits::default()
                },
                ..SessionStatus::default()
            },
        );

        let got = pick_best_session_with_config(&status, Utc::now(), &ScoringConfig::default());
        assert_eq!(got.as_deref(), Some("b"));
    }
}
