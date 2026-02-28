use serde_json::Value;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn write_fake_codex(home: &TempDir) -> Result<PathBuf, Box<dyn Error>> {
    let bin_dir = home.path().join("bin");
    fs::create_dir_all(&bin_dir)?;
    let path = bin_dir.join("codex");
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
cmd="${1:-}"
shift || true
case "$cmd" in
  login)
    exit 0
    ;;
  run)
    if [[ -n "${AGENTLB_RECORD_RUN:-}" ]]; then
      printf '%s\n' "${CODEX_HOME:-}" >> "$AGENTLB_RECORD_RUN"
    fi
    exit 0
    ;;
  app-server)
    mode="${AGENTLB_FAKE_APP_SERVER_MODE:-normal}"
    if [[ "$mode" == "crash" ]]; then
      exit 1
    fi
    while IFS= read -r line; do
      if [[ "$line" == *'"method":"initialize"'* ]]; then
        echo '{"id":1,"result":{"userAgent":"fake"}}'
      fi
      if [[ "$line" == *'"method":"account/rateLimits/read"'* ]]; then
        used="${AGENTLB_FAKE_USED_PERCENT:-15}"
        echo "{\"id\":2,\"result\":{\"rateLimits\":{\"limitId\":\"codex\",\"primary\":{\"usedPercent\":${used},\"windowDurationMins\":15,\"resetsAt\":1730947200},\"secondary\":null}}}"
      fi
    done
    ;;
  *)
    exit 0
    ;;
esac
"#;
    fs::write(&path, script)?;
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)?;
    Ok(path)
}

fn wait_for<F: Fn() -> bool>(timeout: Duration, pred: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn read_json(path: &Path) -> Result<Value, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn kill_supervisor(home: &Path) {
    let pid_path = home.join(".agentlb/supervisor.pid");
    if let Ok(pid_s) = fs::read_to_string(pid_path)
        && let Ok(pid) = pid_s.trim().parse::<i32>()
    {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
}

#[test]
fn supervisor_ingests_rate_limits_and_restarts_managed_process() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;

    fs::create_dir_all(home.path().join(".agentlb/sessions/a"))?;
    fs::create_dir_all(home.path().join(".agentlb/sessions/b"))?;

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake.parent().unwrap().display(),
                std::env::var("PATH")?
            ),
        )
        .env("AGENTLB_FAKE_USED_PERCENT", "12")
        .args(["--cmd", "true"])
        .status()?;
    assert!(status.success());

    let status_path = home.path().join(".agentlb/status.json");
    let ok = wait_for(Duration::from_secs(5), || {
        if !status_path.exists() {
            return false;
        }
        let parsed = read_json(&status_path);
        if let Ok(v) = parsed {
            return v["sessions"]["a"]["rateLimits"]["primary"]["usedPercent"] == 12;
        }
        false
    });
    assert!(ok, "status.json was not populated with rate limits");

    let parsed = read_json(&status_path)?;
    let pid = parsed["sessions"]["a"]["appServer"]["pid"]
        .as_i64()
        .ok_or("missing pid")?;
    assert!(pid > 0);

    let _ = Command::new("kill").arg(pid.to_string()).status()?;

    let restarted = wait_for(Duration::from_secs(6), || {
        if let Ok(v) = read_json(&status_path) {
            let new_pid = v["sessions"]["a"]["appServer"]["pid"].as_i64().unwrap_or(0);
            let rc = v["sessions"]["a"]["appServer"]["restartCount"]
                .as_i64()
                .unwrap_or(0);
            return new_pid > 0 && new_pid != pid && rc >= 1;
        }
        false
    });
    assert!(
        restarted,
        "managed app-server was not restarted as expected"
    );

    kill_supervisor(home.path());
    Ok(())
}

#[test]
fn new_uses_status_scoring_and_fallback_creates_session() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;
    let run_log = home.path().join("run.log");

    fs::create_dir_all(home.path().join(".agentlb/sessions/a"))?;
    fs::create_dir_all(home.path().join(".agentlb/sessions/b"))?;

    let status_json = r#"{
  "version": 1,
  "updatedAt": "2026-02-28T12:00:00Z",
  "daemon": {"pid": 1, "startedAt": "2026-02-28T11:55:00Z"},
  "sessions": {
    "a": {
      "health": "healthy",
      "appServer": {"state": "running", "pid": 11, "restartCount": 0},
      "rateLimits": {"limitId": "codex", "primary": {"usedPercent": 80, "windowDurationMins": 15, "resetsAt": 1730947200}, "secondary": null},
      "usageLeftPercent": 20,
      "activity": {"activeTurns": 0},
      "lastRateLimitUpdateAt": "2099-01-01T00:00:00Z",
      "lastProbeAt": "",
      "lastProbeResult": "",
      "lastSelectedAt": "2026-02-28T11:10:00Z"
    },
    "b": {
      "health": "healthy",
      "appServer": {"state": "running", "pid": 22, "restartCount": 0},
      "rateLimits": {"limitId": "codex", "primary": {"usedPercent": 10, "windowDurationMins": 15, "resetsAt": 1730947200}, "secondary": null},
      "usageLeftPercent": 90,
      "activity": {"activeTurns": 0},
      "lastRateLimitUpdateAt": "2099-01-01T00:00:00Z",
      "lastProbeAt": "",
      "lastProbeResult": "",
      "lastSelectedAt": "2026-02-28T11:10:00Z"
    }
  }
}"#;
    fs::create_dir_all(home.path().join(".agentlb"))?;
    fs::write(home.path().join(".agentlb/status.json"), status_json)?;

    let out = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake.parent().unwrap().display(),
                std::env::var("PATH")?
            ),
        )
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .env("AGENTLB_RECORD_RUN", &run_log)
        .args(["new", "--cmd", "codex run"])
        .output()?;
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("session selection report:"));
    assert!(stderr.contains("selected session: b"));

    let ran = fs::read_to_string(&run_log)?;
    assert!(ran.trim().ends_with("/.agentlb/sessions/b"));

    // Corrupt status => fallback creates new auto session.
    fs::write(home.path().join(".agentlb/status.json"), "not-json")?;
    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake.parent().unwrap().display(),
                std::env::var("PATH")?
            ),
        )
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .args(["new", "--cmd", "true", "--login-cmd", "true"])
        .status()?;
    assert!(status.success());
    assert!(home.path().join(".agentlb/sessions/auto1").exists());

    Ok(())
}

#[test]
fn invoking_agentlb_restarts_dead_supervisor() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;

    fs::create_dir_all(home.path().join(".agentlb/sessions/a"))?;

    let env_path = format!(
        "{}:{}",
        fake.parent().unwrap().display(),
        std::env::var("PATH")?
    );

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("PATH", &env_path)
        .args(["--cmd", "true"])
        .status()?;
    assert!(status.success());

    let pid_path = home.path().join(".agentlb/supervisor.pid");
    let ok = wait_for(Duration::from_secs(5), || pid_path.exists());
    assert!(ok, "supervisor pid file missing");

    let pid1: i32 = fs::read_to_string(&pid_path)?.trim().parse()?;
    let _ = Command::new("kill").arg(pid1.to_string()).status()?;
    thread::sleep(Duration::from_millis(300));

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("PATH", &env_path)
        .args(["--cmd", "true"])
        .status()?;
    assert!(status.success());

    let ok = wait_for(Duration::from_secs(5), || {
        if let Ok(v) = fs::read_to_string(&pid_path)
            && let Ok(pid2) = v.trim().parse::<i32>()
        {
            return pid2 != pid1;
        }
        false
    });
    assert!(ok, "supervisor was not restarted");

    kill_supervisor(home.path());
    Ok(())
}

#[test]
fn startup_probe_updates_unmanaged_sessions() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;
    fs::create_dir_all(home.path().join(".agentlb/sessions/a"))?;

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake.parent().unwrap().display(),
                std::env::var("PATH")?
            ),
        )
        .env("AGENTLB_FAKE_APP_SERVER_MODE", "crash")
        .env("AGENTLB_PROBE_INTERVAL_SEC", "1")
        .env("AGENTLB_PROBE_LIFETIME_SEC", "1")
        .env("AGENTLB_STATUS_FLUSH_INTERVAL_SEC", "1")
        .args(["--cmd", "true"])
        .status()?;
    assert!(status.success());

    let status_path = home.path().join(".agentlb/status.json");
    let ok = wait_for(Duration::from_secs(4), || {
        if let Ok(v) = read_json(&status_path) {
            return v["sessions"]["a"]["lastProbeAt"]
                .as_str()
                .unwrap_or_default()
                != "";
        }
        false
    });
    assert!(ok, "startup probe did not run");

    assert_eq!(
        read_json(&status_path)?["sessions"]["a"]["lastProbeResult"]
            .as_str()
            .unwrap_or_default(),
        "error"
    );

    kill_supervisor(home.path());
    Ok(())
}
