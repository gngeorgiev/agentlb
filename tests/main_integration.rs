use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn config_init_writes_defaults() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .args(["config", "init"])
        .status()?;
    assert!(status.success());

    let cfg = fs::read_to_string(home.path().join(".agentlb/config.toml"))?;
    assert!(cfg.contains("default_command = \"codex\""));
    assert!(cfg.contains("assignment_history_window = 30"));
    assert!(cfg.contains("pick_behavior = \"round_robin\""));
    Ok(())
}

#[test]
fn new_alias_runs_login_and_command_with_session_codex_home() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;
    let login_log = home.path().join("login.log");
    let run_log = home.path().join("run.log");
    let args_log = home.path().join("args.log");

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .env("AGENTLB_RECORD_LOGIN", &login_log)
        .env("AGENTLB_RECORD_RUN", &run_log)
        .env("AGENTLB_RECORD_ARGS", &args_log)
        .args([
            "new",
            "a1",
            "--login-cmd",
            &format!("{} login", fake.display()),
            "--cmd",
            &format!("{} run", fake.display()),
            "--",
            "--search",
        ])
        .status()?;
    assert!(status.success());

    let expected_home = home
        .path()
        .join(".agentlb/sessions/a1")
        .display()
        .to_string();
    assert_eq!(fs::read_to_string(login_log)?.trim(), expected_home);
    assert_eq!(fs::read_to_string(run_log)?.trim(), expected_home);
    assert!(fs::read_to_string(args_log)?.contains("run --search"));

    Ok(())
}

#[test]
fn auto_pick_round_robin_across_aliases() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let fake = write_fake_codex(&home)?;
    let run_log = home.path().join("run.log");

    let cfg_dir = home.path().join(".agentlb");
    fs::create_dir_all(&cfg_dir)?;
    fs::write(
        cfg_dir.join("config.toml"),
        format!(
            "[runner]\ndefault_command = \"{} run\"\ndefault_command_args = []\nlogin_command = \"{} login\"\n\n[sessions]\nalias_pattern = \"^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$\"\nassignment_history_window = 30\npick_behavior = \"round_robin\"\n",
            fake.display(),
            fake.display()
        ),
    )?;

    fs::create_dir_all(home.path().join(".agentlb/sessions/a"))?;
    fs::create_dir_all(home.path().join(".agentlb/sessions/b"))?;

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .env("AGENTLB_RECORD_RUN", &run_log)
        .status()?;
    assert!(status.success());

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .env("AGENTLB_RECORD_RUN", &run_log)
        .status()?;
    assert!(status.success());

    let data = fs::read_to_string(run_log)?;
    let lines: Vec<&str> = data.trim().split('\n').collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        home.path()
            .join(".agentlb/sessions/a")
            .display()
            .to_string()
    );
    assert_eq!(
        lines[1],
        home.path()
            .join(".agentlb/sessions/b")
            .display()
            .to_string()
    );

    Ok(())
}

#[test]
fn supervisor_help_only_prints_usage() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;

    let out1 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor"])
        .output()?;
    assert!(out1.status.success());
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(stdout1.contains("usage:"));
    assert!(!home.path().join(".agentlb/supervisor.pid").exists());
    Ok(())
}

#[test]
fn supervisor_start_background_reports_and_starts() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let out1 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "start", "--background"])
        .output()?;
    assert!(out1.status.success());
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(stdout1.contains("supervisor is not running"));
    assert!(stdout1.contains("started supervisor in background"));

    let pid_path = home.path().join(".agentlb/supervisor.pid");
    let pid1: i32 = fs::read_to_string(&pid_path)?.trim().parse()?;

    let out2 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "start", "--background"])
        .output()?;
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("supervisor is running"));

    let _ = Command::new("kill").arg(pid1.to_string()).status()?;
    Ok(())
}

#[test]
fn supervisor_restart_restarts_pid() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let out1 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "start", "--background"])
        .output()?;
    assert!(out1.status.success());
    let pid_path = home.path().join(".agentlb/supervisor.pid");
    let pid1: i32 = fs::read_to_string(&pid_path)?.trim().parse()?;

    let out2 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "restart"])
        .output()?;
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("supervisor restarted"));

    let pid2: i32 = fs::read_to_string(&pid_path)?.trim().parse()?;
    assert_ne!(pid1, pid2);

    let _ = Command::new("kill").arg(pid2.to_string()).status()?;
    Ok(())
}

#[test]
fn supervisor_stop_stops_running_supervisor() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let out1 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "start", "--background"])
        .output()?;
    assert!(out1.status.success());
    let pid_path = home.path().join(".agentlb/supervisor.pid");
    let pid: i32 = fs::read_to_string(&pid_path)?.trim().parse()?;

    let out2 = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .args(["supervisor", "stop"])
        .output()?;
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("stopping supervisor"));

    let out3 = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()?;
    assert!(!out3.success());
    Ok(())
}

#[test]
fn list_prints_alias_email_and_path() -> Result<(), Box<dyn Error>> {
    let home = TempDir::new()?;
    let sessions = home.path().join(".agentlb/sessions");
    fs::create_dir_all(sessions.join("a"))?;
    fs::create_dir_all(sessions.join("b"))?;

    let payload = r#"{"email":"a@example.com"}"#;
    let token = format!("x.{}.y", URL_SAFE_NO_PAD.encode(payload.as_bytes()));
    fs::write(
        sessions.join("a/auth.json"),
        format!(r#"{{"tokens":{{"id_token":"{}"}}}}"#, token),
    )?;

    let out = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
        .env("AGENTLB_SUPERVISOR_DISABLED", "1")
        .args(["list"])
        .output()?;
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ALIAS"));
    assert!(stdout.contains("EMAIL"));
    assert!(stdout.contains("PATH"));
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines
        .iter()
        .any(|l| l.trim_start().starts_with('a')
            && l.contains("a@example.com")
            && l.contains("/.agentlb/sessions/a")));
    assert!(lines
        .iter()
        .any(|l| l.trim_start().starts_with('b')
            && l.contains(" - ")
            && l.contains("/.agentlb/sessions/b")));
    Ok(())
}

fn write_fake_codex(home: &TempDir) -> Result<PathBuf, Box<dyn Error>> {
    let bin_dir = home.path().join("bin");
    fs::create_dir_all(&bin_dir)?;
    let path = bin_dir.join("fakecodex");
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
cmd="${1:-}"
shift || true
case "$cmd" in
  login)
    if [[ -n "${AGENTLB_RECORD_LOGIN:-}" ]]; then
      printf '%s\n' "${CODEX_HOME:-}" >> "$AGENTLB_RECORD_LOGIN"
    fi
    ;;
  run)
    if [[ -n "${AGENTLB_RECORD_RUN:-}" ]]; then
      printf '%s\n' "${CODEX_HOME:-}" >> "$AGENTLB_RECORD_RUN"
    fi
    if [[ -n "${AGENTLB_RECORD_ARGS:-}" ]]; then
      printf '%s\n' "run $*" >> "$AGENTLB_RECORD_ARGS"
    fi
    ;;
  *)
    ;;
esac
"#;
    fs::write(&path, script)?;
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)?;
    Ok(path)
}
