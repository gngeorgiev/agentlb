mod config;
mod session;
mod state;

use std::io;

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

    if args.mode == "config_init" {
        if let Err(err) = config::write_config_file(&st.config_path, &config::default_config()) {
            eprintln!("{}", err);
            return EXIT_GENERIC;
        }
        println!("{}", st.config_path.display());
        return EXIT_OK;
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
            &args.passthrough,
            &cfg,
            &st,
        ),
        ("new", true) => run_auto_pick(
            &run_cmd,
            &args.passthrough,
            &cfg,
            &st,
            &cfg.sessions.pick_behavior,
        ),
        ("rr", _) => run_auto_pick(&run_cmd, &args.passthrough, &cfg, &st, "round_robin"),
        ("last", _) => run_last(&run_cmd, &args.passthrough, &cfg, &st),
        ("root", _) => run_auto_pick(&run_cmd, &args.passthrough, &cfg, &st, "round_robin"),
        _ => {
            eprintln!("invalid mode");
            EXIT_USAGE
        }
    }
}

fn run_new_alias(
    alias: &str,
    run_cmd: &str,
    login_cmd: &str,
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

    if created && let Err(err) = session::run_login(login_cmd, alias, st) {
        eprintln!("{}", err);
        return EXIT_LOGIN_FAILED;
    }

    match session::run_command(run_cmd, passthrough, alias, st) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", err);
            EXIT_GENERIC
        }
    }
}

fn run_auto_pick(
    run_cmd: &str,
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

fn run_last(run_cmd: &str, passthrough: &[String], cfg: &config::Config, st: &state::Store) -> i32 {
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

fn parse_cli(args: &[String]) -> io::Result<CliArgs> {
    let mut out = CliArgs {
        mode: "root".to_string(),
        ..CliArgs::default()
    };

    let mut i = 0usize;
    while i < args.len() {
        let t = &args[i];
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unexpected argument: {}", t),
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
}
