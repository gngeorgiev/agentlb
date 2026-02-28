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
        .env("AGENTLB_RECORD_RUN", &run_log)
        .status()?;
    assert!(status.success());

    let status = Command::new(env!("CARGO_BIN_EXE_agentlb"))
        .env("HOME", home.path())
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
