use crate::state::{Store, now_rfc3339};
use crate::status::{
    Activity, AppServerStatus, DaemonStatus, RateLimits, RateWindow, SessionStatus, StatusFile,
    load_status, save_status,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_PROBE_INTERVAL_SEC: u64 = 300;
const DEFAULT_PROBE_LIFETIME_SEC: u64 = 10;
const DEFAULT_STATUS_FLUSH_INTERVAL_SEC: u64 = 2;
const DEFAULT_DAEMON_START_TIMEOUT_MS: u64 = 3000;
const DEFAULT_MAX_RESTARTS_5M: usize = 10;

#[cfg(target_family = "unix")]
fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // kill(pid, 0) checks process existence without sending a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(target_family = "unix"))]
fn is_pid_alive(_pid: i32) -> bool {
    false
}

fn read_pid(path: &std::path::Path) -> Option<i32> {
    let data = fs::read_to_string(path).ok()?;
    let pid = data.trim().parse::<i32>().ok()?;
    Some(pid)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SupervisorState {
    pub pid_in_file: Option<i32>,
    pub is_running: bool,
}

pub fn current_state(st: &Store) -> SupervisorState {
    let pid = read_pid(&st.supervisor_pid_path);
    let running = pid.map(is_pid_alive).unwrap_or(false);
    SupervisorState {
        pid_in_file: pid,
        is_running: running,
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn daemon_start_timeout_ms() -> u64 {
    env_u64(
        "AGENTLB_DAEMON_START_TIMEOUT_MS",
        DEFAULT_DAEMON_START_TIMEOUT_MS,
    )
}

pub fn ensure_running(st: &Store) -> io::Result<()> {
    if std::env::var("AGENTLB_SUPERVISOR_DISABLED").unwrap_or_default() == "1" {
        return Ok(());
    }

    if let Some(pid) = read_pid(&st.supervisor_pid_path)
        && is_pid_alive(pid)
    {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let _ = Command::new(exe)
        .args(["supervisor", "daemon"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let deadline = Instant::now() + Duration::from_millis(daemon_start_timeout_ms());
    while Instant::now() < deadline {
        if let Some(pid) = read_pid(&st.supervisor_pid_path)
            && is_pid_alive(pid)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

enum StreamMsg {
    Json(Value),
    Closed,
}

struct ManagedProcess {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<StreamMsg>,
}

struct Runtime {
    managed: Option<ManagedProcess>,
    restart_count: i64,
    restart_attempt: usize,
    restart_times_5m: VecDeque<Instant>,
    next_restart_at: Instant,
    unhealthy_until: Option<Instant>,
}

impl Runtime {
    fn new(now: Instant) -> Self {
        Self {
            managed: None,
            restart_count: 0,
            restart_attempt: 0,
            restart_times_5m: VecDeque::new(),
            next_restart_at: now,
            unhealthy_until: None,
        }
    }
}

pub fn run_daemon(st: &Store) -> io::Result<()> {
    st.ensure_layout()?;

    let me = std::process::id() as i32;
    st.write_text_atomic(&st.supervisor_pid_path, &format!("{}\n", me), 0o600)?;

    let mut status = load_status(st).unwrap_or_else(|_| StatusFile {
        version: 1,
        ..StatusFile::default()
    });
    status.version = 1;
    status.daemon = DaemonStatus {
        pid: me,
        started_at: now_rfc3339(),
    };
    status.updated_at = now_rfc3339();

    let mut runtimes: BTreeMap<String, Runtime> = BTreeMap::new();
    let mut dirty = false;

    let probe_interval = Duration::from_secs(env_u64(
        "AGENTLB_PROBE_INTERVAL_SEC",
        DEFAULT_PROBE_INTERVAL_SEC,
    ));
    let probe_lifetime = Duration::from_secs(env_u64(
        "AGENTLB_PROBE_LIFETIME_SEC",
        DEFAULT_PROBE_LIFETIME_SEC,
    ));
    let flush_interval = Duration::from_secs(env_u64(
        "AGENTLB_STATUS_FLUSH_INTERVAL_SEC",
        DEFAULT_STATUS_FLUSH_INTERVAL_SEC,
    ));
    let max_restarts_5m = env_usize("AGENTLB_MAX_RESTARTS_5M", DEFAULT_MAX_RESTARTS_5M);

    let mut next_probe = Instant::now();
    let mut last_flush = Instant::now() - flush_interval;
    save_status(st, &status)?;

    loop {
        let now = Instant::now();
        let aliases = st.list_aliases()?;

        for alias in &aliases {
            runtimes
                .entry(alias.clone())
                .or_insert_with(|| Runtime::new(now));
            status
                .sessions
                .entry(alias.clone())
                .or_insert_with(default_session_status);
        }

        // Drop missing aliases and terminate managed processes.
        let known: std::collections::BTreeSet<String> = aliases.iter().cloned().collect();
        let stale_aliases: Vec<String> = runtimes
            .keys()
            .filter(|a| !known.contains(*a))
            .cloned()
            .collect();
        for alias in stale_aliases {
            if let Some(mut rt) = runtimes.remove(&alias)
                && let Some(mut mp) = rt.managed.take()
            {
                let _ = mp.child.kill();
                let _ = mp.child.wait();
            }
            status.sessions.remove(&alias);
            dirty = true;
        }

        for alias in &aliases {
            let Some(rt) = runtimes.get_mut(alias) else {
                continue;
            };
            let Some(sess) = status.sessions.get_mut(alias) else {
                continue;
            };

            if let Some(unhealthy_until) = rt.unhealthy_until {
                if now < unhealthy_until {
                    sess.health = "unhealthy".to_string();
                } else {
                    rt.unhealthy_until = None;
                    sess.health = "healthy".to_string();
                }
            }

            // Keep status in sync with live stream.
            if let Some(mp) = rt.managed.as_mut() {
                let mut disconnected = false;
                while let Ok(msg) = mp.rx.try_recv() {
                    match msg {
                        StreamMsg::Json(v) => {
                            handle_message(sess, &v);
                            dirty = true;
                        }
                        StreamMsg::Closed => {
                            disconnected = true;
                        }
                    }
                }

                if disconnected {
                    let _ = mp.child.kill();
                    let _ = mp.child.wait();
                    rt.managed = None;
                    record_restart_failure(rt, sess, now, max_restarts_5m);
                    dirty = true;
                }
            }

            // Detect dead child and schedule restart with backoff.
            let mut dead_now = false;
            if let Some(mp) = rt.managed.as_mut() {
                if let Ok(Some(_)) = mp.child.try_wait() {
                    dead_now = true;
                }
            }
            if dead_now {
                rt.managed = None;
                record_restart_failure(rt, sess, now, max_restarts_5m);
                dirty = true;
            }

            if rt.managed.is_none() && sess.health == "healthy" && now >= rt.next_restart_at {
                match spawn_managed(st, alias) {
                    Ok(mut mp) => {
                        if send_initialize_and_read(&mut mp.stdin).is_err() {
                            let _ = mp.child.kill();
                            let _ = mp.child.wait();
                            record_restart_failure(rt, sess, now, max_restarts_5m);
                        } else {
                            sess.app_server.state = "running".to_string();
                            sess.app_server.pid = mp.child.id() as i32;
                            sess.app_server.restart_count = rt.restart_count;
                            rt.managed = Some(mp);
                            rt.restart_attempt = 0;
                            rt.next_restart_at = now;
                        }
                        dirty = true;
                    }
                    Err(_) => {
                        record_restart_failure(rt, sess, now, max_restarts_5m);
                        dirty = true;
                    }
                }
            }
        }

        // Initial + periodic probe pass only for sessions without active managed connection.
        if now >= next_probe {
            for alias in &aliases {
                let needs_probe = runtimes
                    .get(alias)
                    .map(|rt| rt.managed.is_none())
                    .unwrap_or(true);
                if !needs_probe {
                    continue;
                }
                let (ok, rl) = run_probe(st, alias, probe_lifetime);
                if let Some(sess) = status.sessions.get_mut(alias) {
                    sess.last_probe_at = now_rfc3339();
                    sess.last_probe_result = if ok { "ok" } else { "error" }.to_string();
                    if let Some(rate_limits) = rl {
                        apply_rate_limits(sess, &rate_limits);
                    }
                    dirty = true;
                }
            }
            next_probe = now + probe_interval;
        }

        status.updated_at = now_rfc3339();
        status.daemon.pid = me;

        if dirty && Instant::now().duration_since(last_flush) >= flush_interval {
            save_status(st, &status)?;
            last_flush = Instant::now();
            dirty = false;
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn default_session_status() -> SessionStatus {
    SessionStatus {
        health: "healthy".to_string(),
        app_server: AppServerStatus {
            state: "stopped".to_string(),
            pid: 0,
            restart_count: 0,
        },
        rate_limits: RateLimits {
            limit_id: "codex".to_string(),
            primary: None,
            secondary: None,
        },
        usage_left_percent: 30,
        activity: Activity { active_turns: 0 },
        last_rate_limit_update_at: String::new(),
        last_probe_at: String::new(),
        last_probe_result: String::new(),
        last_selected_at: String::new(),
    }
}

fn schedule_next_restart(rt: &mut Runtime, now: Instant) {
    const BACKOFF: [u64; 7] = [1, 2, 4, 8, 16, 30, 60];
    let idx = rt.restart_attempt.min(BACKOFF.len() - 1);
    let base = BACKOFF[idx];
    rt.restart_attempt = rt.restart_attempt.saturating_add(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let jitter_ms = nanos % 300;
    rt.next_restart_at = now + Duration::from_secs(base) + Duration::from_millis(jitter_ms);
}

fn record_restart_failure(
    rt: &mut Runtime,
    sess: &mut SessionStatus,
    now: Instant,
    max_restarts_5m: usize,
) {
    rt.restart_count += 1;
    sess.app_server.restart_count = rt.restart_count;
    sess.app_server.state = "stopped".to_string();
    sess.app_server.pid = 0;

    rt.restart_times_5m.push_back(now);
    while let Some(front) = rt.restart_times_5m.front() {
        if now.duration_since(*front) > Duration::from_secs(300) {
            let _ = rt.restart_times_5m.pop_front();
        } else {
            break;
        }
    }
    if rt.restart_times_5m.len() > max_restarts_5m {
        rt.unhealthy_until = Some(now + Duration::from_secs(300));
        sess.health = "unhealthy".to_string();
    }

    schedule_next_restart(rt, now);
}

fn spawn_managed(st: &Store, alias: &str) -> io::Result<ManagedProcess> {
    let mut child = Command::new("codex")
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", st.session_dir(alias))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("managed app-server stdin missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("managed app-server stdout missing"))?;

    let (tx, rx) = mpsc::channel::<StreamMsg>();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(StreamMsg::Json(v));
            }
        }
        let _ = tx.send(StreamMsg::Closed);
    });

    Ok(ManagedProcess { child, stdin, rx })
}

fn send_initialize_and_read(stdin: &mut ChildStdin) -> io::Result<()> {
    let init = json!({
      "jsonrpc": "2.0",
      "id": 1,
      "method": "initialize",
      "params": {
        "clientInfo": {
          "name": "agentlb-supervisor",
          "version": env!("CARGO_PKG_VERSION")
        }
      }
    });
    let read = json!({
      "jsonrpc": "2.0",
      "id": 2,
      "method": "account/rateLimits/read",
      "params": {}
    });
    writeln!(stdin, "{}", init)?;
    writeln!(stdin, "{}", read)?;
    stdin.flush()
}

fn run_probe(st: &Store, alias: &str, lifetime: Duration) -> (bool, Option<Value>) {
    let mut child = match Command::new("codex")
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", st.session_dir(alias))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return (false, None),
    };

    let mut ok = false;
    let mut found_rate_limits: Option<Value> = None;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = send_initialize_and_read(&mut stdin);
    }

    let deadline = Instant::now() + lifetime;
    if let Some(stdout) = child.stdout.take() {
        let (tx, rx) = mpsc::channel::<Value>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let _ = tx.send(v);
                }
            }
        });

        while Instant::now() < deadline {
            if let Ok(v) = rx.recv_timeout(Duration::from_millis(200)) {
                if let Some(rl) = extract_rate_limits(&v) {
                    found_rate_limits = Some(rl);
                    ok = true;
                    break;
                }
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    (ok, found_rate_limits)
}

fn handle_message(sess: &mut SessionStatus, msg: &Value) {
    if let Some(rl) = extract_rate_limits(msg) {
        apply_rate_limits(sess, &rl);
    }

    let method = msg
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "turn/started" => {
            sess.activity.active_turns += 1;
        }
        "turn/completed" => {
            sess.activity.active_turns = (sess.activity.active_turns - 1).max(0);
        }
        "thread/status/changed" => {
            if let Some(status_type) = params
                .get("status")
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
            {
                if status_type == "active" {
                    sess.health = "healthy".to_string();
                } else if status_type == "systemError" {
                    sess.health = "unhealthy".to_string();
                }
            }
        }
        _ => {}
    }
}

fn extract_rate_limits(msg: &Value) -> Option<Value> {
    if let Some(method) = msg.get("method").and_then(|v| v.as_str())
        && method == "account/rateLimits/updated"
    {
        return msg.get("params").and_then(|p| p.get("rateLimits")).cloned();
    }

    msg.get("result").and_then(|r| r.get("rateLimits")).cloned()
}

fn apply_rate_limits(sess: &mut SessionStatus, rl: &Value) {
    sess.rate_limits.limit_id = rl
        .get("limitId")
        .and_then(|v| v.as_str())
        .unwrap_or("codex")
        .to_string();

    sess.rate_limits.primary = to_window(rl.get("primary"));
    sess.rate_limits.secondary = to_window(rl.get("secondary"));

    let mut mins: Vec<i64> = vec![];
    if let Some(p) = &sess.rate_limits.primary {
        mins.push((100 - p.used_percent).clamp(0, 100));
    }
    if let Some(s) = &sess.rate_limits.secondary {
        mins.push((100 - s.used_percent).clamp(0, 100));
    }
    sess.usage_left_percent = mins.into_iter().min().unwrap_or(30);
    sess.last_rate_limit_update_at = now_rfc3339();
}

fn to_window(v: Option<&Value>) -> Option<RateWindow> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let used = v.get("usedPercent").and_then(|x| x.as_i64())?;
    Some(RateWindow {
        used_percent: used,
        window_duration_mins: v
            .get("windowDurationMins")
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        resets_at: v.get("resetsAt").and_then(|x| x.as_i64()).unwrap_or(0),
    })
}
