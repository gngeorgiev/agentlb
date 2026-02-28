mod config;
mod session;
mod state;
mod status;
mod supervisor;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use comfy_table::{Cell, ContentArrangement, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use std::io;
use std::path::Path;

const EXIT_OK: i32 = 0;
const EXIT_GENERIC: i32 = 1;
const EXIT_USAGE: i32 = 2;
#[allow(dead_code)]
const EXIT_ALIAS_NOT_FOUND: i32 = 3;
const EXIT_NO_SESSIONS: i32 = 4;
const EXIT_LOGIN_FAILED: i32 = 5;

#[derive(Debug, Default)]
struct CliArgs {
    mode: String,
    alias: String,
    cmd: String,
    login_cmd: String,
    print_command: bool,
    supervisor_background: bool,
    passthrough: Vec<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(args));
}

fn run(argv: Vec<String>) -> i32 {
    let args = match parse_cli(&argv) {
        Ok(a) => a,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_USAGE;
        }
    };

    if args.mode == "help" {
        print_help();
        return EXIT_OK;
    }

    let st = match state::Store::new() {
        Ok(st) => st,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
    };
    if let Err(err) = st.ensure_layout() {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    if args.mode == "supervisor_daemon" {
        if let Err(err) = supervisor::run_daemon(&st) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
        return EXIT_OK;
    }
    if args.mode == "supervisor_help" {
        print_supervisor_help();
        return EXIT_OK;
    }
    if args.mode == "supervisor_start" {
        if !args.supervisor_background {
            eprintln!("usage: agentlb supervisor start --background");
            return EXIT_USAGE;
        }
        return run_supervisor_background(&st);
    }
    if args.mode == "supervisor_restart" {
        return run_supervisor_restart(&st);
    }
    if args.mode == "supervisor_stop" {
        return run_supervisor_stop(&st);
    }

    if args.mode == "config_init" {
        if let Err(err) = config::write_config_file(&st.config_path, &config::default_config()) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
        println!("{}", st.config_path.display());
        return EXIT_OK;
    }
    if args.mode == "status" {
        return run_status(&st);
    }
    if args.mode == "rm" {
        return run_rm(&args.alias, &st);
    }

    if let Err(err) = supervisor::ensure_running(&st) {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    let cfg = match config::load(&st.config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
    };

    let run_cmd = resolve_run_command(&args, &cfg);
    let login_cmd = resolve_login_command(&args, &cfg, &run_cmd);

    match (args.mode.as_str(), args.alias.is_empty()) {
        ("new", false) => run_new_alias(
            &args.alias,
            &run_cmd,
            &login_cmd,
            args.print_command,
            &args.passthrough,
            &cfg,
            &st,
        ),
        ("new", true) => {
            run_new_with_status_pick(&run_cmd, args.print_command, &args.passthrough, &cfg, &st)
        }
        ("rr", _) => run_auto_pick(
            &run_cmd,
            args.print_command,
            &args.passthrough,
            &cfg,
            &st,
            "round_robin",
        ),
        ("last", _) => run_last(&run_cmd, args.print_command, &args.passthrough, &cfg, &st),
        ("root", _) => run_auto_pick(
            &run_cmd,
            args.print_command,
            &args.passthrough,
            &cfg,
            &st,
            "round_robin",
        ),
        _ => {
            eprintln!("invalid mode");
            EXIT_USAGE
        }
    }
}

fn run_supervisor_background(st: &state::Store) -> i32 {
    let before = supervisor::current_state(st);
    if before.is_running {
        if let Some(pid) = before.pid_in_file {
            println!("supervisor is running (pid {})", pid);
        } else {
            println!("supervisor is running");
        }
        return EXIT_OK;
    }

    match before.pid_in_file {
        Some(pid) => println!("supervisor is not running (stale pid {})", pid),
        None => println!("supervisor is not running"),
    }

    if let Err(err) = supervisor::ensure_running(st) {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    let after = supervisor::current_state(st);
    if after.is_running {
        if let Some(pid) = after.pid_in_file {
            println!("started supervisor in background (pid {})", pid);
        } else {
            println!("started supervisor in background");
        }
    } else {
        println!("attempted to start supervisor in background");
    }
    EXIT_OK
}

fn run_supervisor_restart(st: &state::Store) -> i32 {
    let before = supervisor::current_state(st);
    if before.is_running {
        if let Some(pid) = before.pid_in_file {
            println!("stopping supervisor (pid {})", pid);
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if !supervisor::current_state(st).is_running {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    } else if let Some(pid) = before.pid_in_file {
        println!("supervisor not running (stale pid {})", pid);
    } else {
        println!("supervisor not running");
    }

    if let Err(err) = supervisor::ensure_running(st) {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }
    let after = supervisor::current_state(st);
    if after.is_running {
        if let Some(pid) = after.pid_in_file {
            println!("supervisor restarted (pid {})", pid);
        } else {
            println!("supervisor restarted");
        }
    } else {
        println!("attempted supervisor restart");
    }
    EXIT_OK
}

fn run_supervisor_stop(st: &state::Store) -> i32 {
    let before = supervisor::current_state(st);
    if before.is_running {
        if let Some(pid) = before.pid_in_file {
            println!("stopping supervisor (pid {})", pid);
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                if !supervisor::current_state(st).is_running {
                    println!("supervisor stopped");
                    return EXIT_OK;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            println!("supervisor stop requested");
            return EXIT_OK;
        }
        println!("supervisor running");
        return EXIT_OK;
    }

    if let Some(pid) = before.pid_in_file {
        println!("supervisor already stopped (stale pid {})", pid);
    } else {
        println!("supervisor already stopped");
    }
    EXIT_OK
}

fn print_supervisor_help() {
    println!("usage:");
    println!("  agentlb supervisor");
    println!("  agentlb supervisor start --background");
    println!("  agentlb supervisor restart");
    println!("  agentlb supervisor stop");
}

fn print_help() {
    println!("usage:");
    println!("  agentlb [--print-command] [--cmd <command>] [-- <args...>]");
    println!("  agentlb status");
    println!(
        "  agentlb new [<alias-or-email>] [--print-command] [--cmd <command>] [--login-cmd <command>] [-- <args...>]"
    );
    println!("  agentlb rm <alias-or-email>");
    println!("  agentlb rr [--print-command] [--cmd <command>] [-- <args...>]");
    println!("  agentlb last [--print-command] [--cmd <command>] [-- <args...>]");
    println!("  agentlb supervisor");
    println!("  agentlb supervisor start --background");
    println!("  agentlb supervisor restart");
    println!("  agentlb supervisor stop");
    println!("  agentlb config init");
}

fn run_rm(target: &str, st: &state::Store) -> i32 {
    let alias = match resolve_alias_target(target, st) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_USAGE;
        }
    };

    let _lock = match st.lock() {
        Ok(l) => l,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
    };

    let session_dir = st.session_dir(&alias);
    if !session_dir.exists() {
        eprintln!("session {:?} does not exist", alias);
        return EXIT_USAGE;
    }

    if let Err(err) = std::fs::remove_dir_all(&session_dir) {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    let ss_path = st.session_state_path(&alias);
    if let Err(err) = std::fs::remove_file(&ss_path)
        && err.kind() != io::ErrorKind::NotFound
    {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    match st.load_global() {
        Ok(mut g) => {
            if g.last_alias == alias {
                g.last_alias.clear();
                if let Err(err) = st.save_global(&g) {
                    eprintln!("{}", err);
                    return EXIT_GENERIC;
                }
            }
        }
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
    }

    if let Ok(mut sf) = status::load_status(st) {
        sf.sessions.remove(&alias);
        sf.updated_at = state::now_rfc3339();
        if let Err(err) = status::save_status(st, &sf) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
    }

    if std::env::var("AGENTLB_SUPERVISOR_DISABLED").unwrap_or_default() != "1"
        && let Err(err) = restart_supervisor_for_rm(st)
    {
        eprintln!("{}", err);
        return EXIT_GENERIC;
    }

    println!("removed session: {}", alias);
    EXIT_OK
}

fn restart_supervisor_for_rm(st: &state::Store) -> io::Result<()> {
    let before = supervisor::current_state(st);
    if before.is_running
        && let Some(pid) = before.pid_in_file
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if !supervisor::current_state(st).is_running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    supervisor::ensure_running(st)
}

fn run_status(st: &state::Store) -> i32 {
    let cfg = config::load(&st.config_path).unwrap_or_else(|_| config::default_config());
    let scoring_cfg = status::ScoringConfig::from(&cfg.sessions);
    match status::load_status(st) {
        Ok(sf) => {
            let selected =
                status::pick_best_session_with_config(&sf, chrono::Utc::now(), &scoring_cfg);
            print_unified_status_table(st, &sf, &scoring_cfg, selected.as_deref());
        }
        Err(err) => {
            eprintln!("session selection report: unavailable ({})", err);
        }
    }
    EXIT_OK
}

fn read_session_email(session_dir: &Path) -> Option<String> {
    let auth_path = session_dir.join("auth.json");
    let bytes = std::fs::read(auth_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    if let Some(v) = extract_email(&value) {
        return Some(v);
    }
    let id_token = value
        .get("tokens")
        .and_then(|v| v.get("id_token"))
        .and_then(|v| v.as_str())?;
    decode_jwt_email(id_token)
}

fn extract_email(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(email) = map.get("email").and_then(|e| e.as_str())
                && email.contains('@')
            {
                return Some(email.to_string());
            }
            for value in map.values() {
                if let Some(email) = extract_email(value) {
                    return Some(email);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(email) = extract_email(item) {
                    return Some(email);
                }
            }
            None
        }
        _ => None,
    }
}

fn decode_jwt_email(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    extract_email(&value)
}

fn run_new_alias(
    target: &str,
    run_cmd: &str,
    login_cmd: &str,
    print_command: bool,
    passthrough: &[String],
    cfg: &config::Config,
    st: &state::Store,
) -> i32 {
    let alias = match resolve_alias_target(target, st) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{}", err);
            return EXIT_USAGE;
        }
    };

    run_new_alias_resolved(&alias, run_cmd, login_cmd, print_command, passthrough, cfg, st)
}

fn run_new_alias_resolved(
    alias: &str,
    run_cmd: &str,
    login_cmd: &str,
    print_command: bool,
    passthrough: &[String],
    cfg: &config::Config,
    st: &state::Store,
) -> i32 {
    if let Err(err) = session::validate_alias(alias, &cfg.sessions.alias_pattern) {
        eprintln!("{}", err);
        return EXIT_USAGE;
    }

    let created = {
        let _lock = match st.lock() {
            Ok(l) => l,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        let created = match session::ensure_session_dir(st, alias) {
            Ok(created) => created,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        if let Err(err) = config::ensure_default_config_file(&st.config_path, cfg) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        if let Err(err) = st.record_assignment(alias, cfg.sessions.assignment_history_window) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        let mut g = match st.load_global() {
            Ok(g) => g,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };
        g.last_alias = alias.to_string();
        if let Err(err) = st.save_global(&g) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        created
    };

    if !print_command {
        if created && let Err(err) = session::run_login(login_cmd, alias, st) {
            eprintln!("{}", err);
            return EXIT_LOGIN_FAILED;
        }
    } else {
        return print_shell_command(run_cmd, passthrough, alias, st);
    }

    match session::run_command(run_cmd, passthrough, alias, st) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err);
            EXIT_GENERIC
        }
    }
}

fn resolve_alias_target(target: &str, st: &state::Store) -> io::Result<String> {
    if !target.contains('@') {
        return Ok(target.to_string());
    }

    let aliases = st.list_aliases()?;
    let mut matches: Vec<String> = aliases
        .iter()
        .filter(|a| !a.starts_with('.'))
        .filter(|alias| {
            let path = st.session_dir(alias);
            read_session_email(&path)
                .map(|e| e.eq_ignore_ascii_case(target))
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    matches.sort();
    matches.dedup();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no session found for email {:?}; create one with: agentlb new <alias>",
                target
            ),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "multiple sessions match email {:?}: {}; use alias explicitly",
                target,
                matches.join(", ")
            ),
        )),
    }
}

fn run_new_with_status_pick(
    run_cmd: &str,
    print_command: bool,
    passthrough: &[String],
    cfg: &config::Config,
    st: &state::Store,
) -> i32 {
    let scoring_cfg = status::ScoringConfig::from(&cfg.sessions);
    let winner = status::pick_best_session_with_retry_and_config(st, 3000, &scoring_cfg);
    if let Ok(sf) = status::load_status(st) {
        let report = render_unified_status_table(st, &sf, &scoring_cfg, winner.as_deref());
        print!("{}", report);
    }
    if let Some(winner) = winner {
        let winner_exists = {
            let _lock = match st.lock() {
                Ok(l) => l,
                Err(err) => {
                    eprintln!("{}", err);
                    return EXIT_GENERIC;
                }
            };

            let aliases = match st.list_aliases() {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("{}", err);
                    return EXIT_GENERIC;
                }
            };
            if !aliases.iter().any(|a| a == &winner) {
                false
            } else {
                let mut g = match st.load_global() {
                    Ok(g) => g,
                    Err(err) => {
                        eprintln!("{}", err);
                        return EXIT_GENERIC;
                    }
                };
                g.last_alias = winner.clone();
                if let Err(err) = st.save_global(&g) {
                    eprintln!("{}", err);
                    return EXIT_GENERIC;
                }
                if let Err(err) =
                    st.record_assignment(&winner, cfg.sessions.assignment_history_window)
                {
                    eprintln!("{}", err);
                    return EXIT_GENERIC;
                }
                true
            }
        };

        if winner_exists {
            status::mark_selected(st, &winner);
            if print_command {
                return print_shell_command(run_cmd, passthrough, &winner, st);
            }
            return match session::run_command(run_cmd, passthrough, &winner, st) {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("{}", err);
                    EXIT_GENERIC
                }
            };
        }

        eprintln!(
            "selected session {:?} not found; falling back to managed aliases",
            winner
        );
        return run_auto_pick(run_cmd, print_command, passthrough, cfg, st, "round_robin");
    }

    run_auto_pick(run_cmd, print_command, passthrough, cfg, st, "round_robin")
}

fn render_unified_status_table(
    st: &state::Store,
    status_file: &status::StatusFile,
    scoring_cfg: &status::ScoringConfig,
    selected: Option<&str>,
) -> String {
    let rows = status::score_rows_with_config(status_file, chrono::Utc::now(), scoring_cfg);
    use std::collections::{BTreeMap, BTreeSet};

    let scored = rows;
    let mut score_by_alias: BTreeMap<String, status::SessionScoreRow> = BTreeMap::new();
    for row in scored {
        score_by_alias.insert(row.alias.clone(), row);
    }

    let fs_aliases = st.list_aliases().unwrap_or_default();
    let mut aliases: BTreeSet<String> = BTreeSet::new();
    for a in fs_aliases.into_iter().filter(|a| !a.starts_with('.')) {
        aliases.insert(a);
    }
    for a in score_by_alias.keys().filter(|a| !a.starts_with('.')) {
        aliases.insert(a.clone());
    }

    let mut meta: BTreeMap<String, (String, String)> = BTreeMap::new();
    for alias in &aliases {
        let path = st.session_dir(alias);
        let email = read_session_email(&path).unwrap_or_else(|| "-".to_string());
        meta.insert(alias.clone(), (email, path.display().to_string()));
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Disabled)
        .set_header(vec![
            Cell::new("SEL"),
            Cell::new("ALIAS"),
            Cell::new("EMAIL"),
            Cell::new("PATH"),
            Cell::new("PRIM"),
            Cell::new("WEEK"),
            Cell::new("USAGE"),
            Cell::new("REFRESH"),
            Cell::new("SCORE"),
            Cell::new("RESTART"),
            Cell::new("ACTIVE"),
            Cell::new("HEALTH"),
            Cell::new("REASON"),
        ]);

    for alias in aliases {
        let marker = if selected == Some(alias.as_str()) {
            "*"
        } else {
            " "
        };
        let (email, path) = meta
            .get(&alias)
            .cloned()
            .unwrap_or_else(|| ("-".to_string(), "-".to_string()));
        if let Some(row) = score_by_alias.get(&alias) {
            let prim = row
                .primary_left
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string());
            let week = row
                .secondary_left
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string());
            let score = row
                .score
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string());
            let refresh = format_age(row.refresh_age_sec);
            table.add_row(vec![
                Cell::new(marker),
                Cell::new(alias),
                Cell::new(email),
                Cell::new(path),
                Cell::new(prim),
                Cell::new(week),
                Cell::new(row.usage_left),
                Cell::new(refresh),
                Cell::new(score),
                Cell::new(row.restart_count),
                Cell::new(row.active_turns),
                Cell::new(&row.health),
                Cell::new(&row.reason),
            ]);
        } else {
            table.add_row(vec![
                Cell::new(marker),
                Cell::new(alias),
                Cell::new(email),
                Cell::new(path),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("-"),
                Cell::new("unknown"),
                Cell::new("missing status"),
            ]);
        }
    }

    let mut out = format!("{}\n", table);
    if let Some(sel) = selected {
        out.push_str(&format!("selected session: {}\n", sel));
    } else {
        out.push_str("selected session: <none>\n");
    }
    out
}

fn print_unified_status_table(
    st: &state::Store,
    status_file: &status::StatusFile,
    scoring_cfg: &status::ScoringConfig,
    selected: Option<&str>,
) {
    let out = render_unified_status_table(st, status_file, scoring_cfg, selected);
    print!("{}", out);
}

fn run_auto_pick(
    run_cmd: &str,
    print_command: bool,
    passthrough: &[String],
    cfg: &config::Config,
    st: &state::Store,
    pick_behavior: &str,
) -> i32 {
    let winner = {
        let _lock = match st.lock() {
            Ok(l) => l,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        let aliases = match st.list_aliases() {
            Ok(aliases) => aliases,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        if aliases.is_empty() {
            eprintln!("no managed sessions found; create one with: agentlb new <alias>");
            return EXIT_NO_SESSIONS;
        }

        let mut g = match st.load_global() {
            Ok(g) => g,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        let winner = match pick_alias(&aliases, &mut g, pick_behavior) {
            Ok(winner) => winner,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_NO_SESSIONS;
            }
        };

        g.last_alias = winner.clone();

        if let Err(err) = config::ensure_default_config_file(&st.config_path, cfg) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        if let Err(err) = st.record_assignment(&winner, cfg.sessions.assignment_history_window) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        if let Err(err) = st.save_global(&g) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        winner
    };

    if print_command {
        return print_shell_command(run_cmd, passthrough, &winner, st);
    }

    match session::run_command(run_cmd, passthrough, &winner, st) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err);
            EXIT_GENERIC
        }
    }
}

fn pick_alias(
    aliases: &[String],
    g: &mut state::GlobalState,
    pick_behavior: &str,
) -> io::Result<String> {
    match pick_behavior {
        "last" => {
            if g.last_alias.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no previous session recorded; run agentlb or agentlb new <alias> first",
                ));
            }
            if aliases.iter().any(|a| a == &g.last_alias) {
                Ok(g.last_alias.clone())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "last session no longer exists; run agentlb or agentlb new <alias> first",
                ))
            }
        }
        _ => {
            let winner = aliases[g.round_robin_index % aliases.len()].clone();
            g.round_robin_index = (g.round_robin_index + 1) % aliases.len();
            Ok(winner)
        }
    }
}

fn run_last(
    run_cmd: &str,
    print_command: bool,
    passthrough: &[String],
    cfg: &config::Config,
    st: &state::Store,
) -> i32 {
    let alias = {
        let _lock = match st.lock() {
            Ok(l) => l,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        let aliases = match st.list_aliases() {
            Ok(aliases) => aliases,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };
        if aliases.is_empty() {
            eprintln!("no managed sessions found; create one with: agentlb new <alias>");
            return EXIT_NO_SESSIONS;
        }

        let g = match st.load_global() {
            Ok(g) => g,
            Err(err) => {
                eprintln!("{}", err);
                return EXIT_GENERIC;
            }
        };

        if g.last_alias.is_empty() {
            eprintln!("no previous session recorded; run agentlb or agentlb new <alias> first");
            return EXIT_NO_SESSIONS;
        }
        if !aliases.iter().any(|a| a == &g.last_alias) {
            eprintln!("last session no longer exists; run agentlb or agentlb new <alias> first");
            return EXIT_NO_SESSIONS;
        }

        if let Err(err) = config::ensure_default_config_file(&st.config_path, cfg) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
        if let Err(err) =
            st.record_assignment(&g.last_alias, cfg.sessions.assignment_history_window)
        {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
        if let Err(err) = st.save_global(&g) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }

        g.last_alias
    };

    if print_command {
        return print_shell_command(run_cmd, passthrough, &alias, st);
    }

    match session::run_command(run_cmd, passthrough, &alias, st) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err);
            EXIT_GENERIC
        }
    }
}

fn resolve_run_command(a: &CliArgs, cfg: &config::Config) -> String {
    if !a.cmd.is_empty() {
        return a.cmd.clone();
    }
    let mut parts = vec![cfg.runner.default_command.clone()];
    parts.extend(cfg.runner.default_command_args.clone());
    parts.join(" ").trim().to_string()
}

fn resolve_login_command(a: &CliArgs, cfg: &config::Config, run_cmd: &str) -> String {
    if !a.login_cmd.is_empty() {
        return a.login_cmd.clone();
    }
    if !cfg.runner.login_command.trim().is_empty() {
        return cfg.runner.login_command.clone();
    }
    match config::split_command(run_cmd) {
        Ok((bin, _)) => format!("{} login", bin),
        Err(_) => "codex login".to_string(),
    }
}

fn print_shell_command(run_cmd: &str, passthrough: &[String], alias: &str, st: &state::Store) -> i32 {
    match session::render_shell_command(run_cmd, passthrough, alias, st) {
        Ok(cmd) => {
            println!("{}", cmd);
            EXIT_OK
        }
        Err(err) => {
            eprintln!("{}", err);
            EXIT_GENERIC
        }
    }
}

fn parse_cli(args: &[String]) -> io::Result<CliArgs> {
    let mut out = CliArgs {
        mode: "root".to_string(),
        ..CliArgs::default()
    };

    let mut i = 0usize;
    while i < args.len() {
        let t = &args[i];
        if (t == "--help" || t == "-h") && out.mode == "root" {
            out.mode = "help".to_string();
            i += 1;
            continue;
        }
        if t == "--" {
            out.passthrough.extend_from_slice(&args[i + 1..]);
            break;
        }

        match t.as_str() {
            "new" => {
                if out.mode == "root" {
                    out.mode = "new".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "rm" => {
                if out.mode == "root" {
                    out.mode = "rm".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "status" => {
                if out.mode == "root" {
                    out.mode = "status".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "rr" => {
                if out.mode == "root" {
                    out.mode = "rr".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "last" => {
                if out.mode == "root" {
                    out.mode = "last".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "supervisor" => {
                if out.mode == "root" {
                    out.mode = "supervisor_help".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "start" => {
                if out.mode == "supervisor_help" {
                    out.mode = "supervisor_start".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "restart" => {
                if out.mode == "supervisor_help" {
                    out.mode = "supervisor_restart".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "stop" => {
                if out.mode == "supervisor_help" {
                    out.mode = "supervisor_stop".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "daemon" => {
                if out.mode == "supervisor_help" {
                    out.mode = "supervisor_daemon".to_string();
                    i += 1;
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid usage"));
            }
            "config" => {
                if out.mode != "root" || i + 1 >= args.len() || args[i + 1] != "init" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "usage: agentlb config init",
                    ));
                }
                out.mode = "config_init".to_string();
                i += 2;
                continue;
            }
            "--cmd" => {
                i += 1;
                if i >= args.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--cmd requires a value",
                    ));
                }
                out.cmd = args[i].clone();
                i += 1;
                continue;
            }
            "--login-cmd" => {
                i += 1;
                if i >= args.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--login-cmd requires a value",
                    ));
                }
                out.login_cmd = args[i].clone();
                i += 1;
                continue;
            }
            "--background" => {
                if out.mode != "supervisor_start" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "unexpected --background",
                    ));
                }
                out.supervisor_background = true;
                i += 1;
                continue;
            }
            "--print-command" => {
                out.print_command = true;
                i += 1;
                continue;
            }
            "--help" | "-h" => {
                if out.mode == "supervisor_help" {
                    out.mode = "supervisor_help".to_string();
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }

        if let Some(v) = t.strip_prefix("--cmd=") {
            out.cmd = v.to_string();
            i += 1;
            continue;
        }
        if let Some(v) = t.strip_prefix("--login-cmd=") {
            out.login_cmd = v.to_string();
            i += 1;
            continue;
        }
        if t.starts_with('-') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown flag: {}", t),
            ));
        }
        if out.mode == "new" && out.alias.is_empty() {
            out.alias = t.clone();
            i += 1;
            continue;
        }
        if out.mode == "rm" && out.alias.is_empty() {
            out.alias = t.clone();
            i += 1;
            continue;
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unexpected argument: {}", t),
        ));
    }

    if out.mode == "rm" && out.alias.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: agentlb rm <alias-or-email>",
        ));
    }

    if out.print_command && !matches!(out.mode.as_str(), "root" | "new" | "rr" | "last") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--print-command is only supported for session run commands",
        ));
    }

    if out.mode == "root" && !out.alias.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected alias",
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_cli_rejects_unknown_flag() {
        let args = vec!["--nope".to_string()];
        assert!(super::parse_cli(&args).is_err());
    }

    #[test]
    fn pick_alias_last_requires_existing() {
        let aliases = vec!["a".to_string(), "b".to_string()];
        let mut g = crate::state::GlobalState {
            last_alias: "c".to_string(),
            ..Default::default()
        };
        assert!(super::pick_alias(&aliases, &mut g, "last").is_err());
    }

    #[test]
    fn keep_exit_code_alias_not_found_reserved() {
        assert_eq!(super::EXIT_ALIAS_NOT_FOUND, 3);
    }

    #[test]
    fn parse_cli_accepts_supervisor_background() {
        let args = vec![
            "supervisor".to_string(),
            "start".to_string(),
            "--background".to_string(),
        ];
        let parsed = super::parse_cli(&args).expect("parse should succeed");
        assert_eq!(parsed.mode, "supervisor_start");
        assert!(parsed.supervisor_background);
    }

    #[test]
    fn parse_cli_supervisor_without_subcommand_shows_help_mode() {
        let args = vec!["supervisor".to_string()];
        let parsed = super::parse_cli(&args).expect("parse should succeed");
        assert_eq!(parsed.mode, "supervisor_help");
    }

    #[test]
    fn parse_cli_accepts_global_help_flag() {
        let args = vec!["--help".to_string()];
        let parsed = super::parse_cli(&args).expect("parse should succeed");
        assert_eq!(parsed.mode, "help");
    }

    #[test]
    fn parse_cli_status_command() {
        let args = vec!["status".to_string()];
        let parsed = super::parse_cli(&args).expect("parse should succeed");
        assert_eq!(parsed.mode, "status");
    }

    #[test]
    fn parse_cli_accepts_print_command_flag() {
        let args = vec!["new".to_string(), "--print-command".to_string()];
        let parsed = super::parse_cli(&args).expect("parse should succeed");
        assert_eq!(parsed.mode, "new");
        assert!(parsed.print_command);
    }

    #[test]
    fn parse_cli_rejects_print_command_for_status() {
        let args = vec!["status".to_string(), "--print-command".to_string()];
        assert!(super::parse_cli(&args).is_err());
    }

    #[test]
    fn parse_cli_rm_requires_target() {
        let args = vec!["rm".to_string()];
        assert!(super::parse_cli(&args).is_err());
    }
}
    fn format_age(age_sec: Option<i64>) -> String {
        let Some(sec) = age_sec else {
            return "-".to_string();
        };
        if sec < 60 {
            return format!("{}s", sec);
        }
        if sec < 3600 {
            return format!("{}m", sec / 60);
        }
        if sec < 86_400 {
            return format!("{}h", sec / 3600);
        }
        format!("{}d", sec / 86_400)
    }
