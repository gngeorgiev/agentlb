use regex::Regex;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn validate_alias(alias: &str, pattern: &str) -> io::Result<()> {
    let re = Regex::new(pattern).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid alias pattern {:?}: {}", pattern, err),
        )
    })?;
    if !re.is_match(alias) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "invalid alias {:?}; must match {} (example: a1)",
                alias, pattern
            ),
        ));
    }
    Ok(())
}

pub fn ensure_session_dir(st: &crate::state::Store, alias: &str) -> io::Result<bool> {
    let dir = st.session_dir(alias);
    if dir.exists() {
        return Ok(false);
    }
    fs::create_dir_all(&dir)?;
    crate::state::set_mode(&dir, 0o700)?;
    Ok(true)
}

pub fn run_login(login_cmd: &str, alias: &str, st: &crate::state::Store) -> io::Result<()> {
    let (bin, args) = crate::config::split_command(login_cmd)?;
    let status = command_with_codex_home(Path::new(&bin), &args, alias, st)?.status()?;
    if !status.success() {
        if let Some(code) = status.code() {
            return Err(io::Error::other(format!(
                "login command failed: {:?} (exit {})",
                login_cmd, code
            )));
        }
        return Err(io::Error::other(format!(
            "login command failed: {:?}",
            login_cmd
        )));
    }
    Ok(())
}

pub fn run_command(
    run_cmd: &str,
    passthrough: &[String],
    alias: &str,
    st: &crate::state::Store,
) -> io::Result<i32> {
    let (bin, mut args) = crate::config::split_command(run_cmd)?;
    args.extend_from_slice(passthrough);
    let status = command_with_codex_home(Path::new(&bin), &args, alias, st)?.status()?;
    Ok(status.code().unwrap_or(1))
}

pub fn render_shell_command(
    run_cmd: &str,
    passthrough: &[String],
    alias: &str,
    st: &crate::state::Store,
) -> io::Result<String> {
    let (bin, mut args) = crate::config::split_command(run_cmd)?;
    args.extend_from_slice(passthrough);
    let mut parts = Vec::with_capacity(args.len() + 2);
    let codex_home = st.session_dir(alias).display().to_string();
    parts.push(format!("CODEX_HOME={}", shell_quote(&codex_home)));
    parts.push(shell_quote(&bin));
    for arg in args {
        parts.push(shell_quote(&arg));
    }
    Ok(parts.join(" "))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_./:=+".contains(&b))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn command_with_codex_home(
    bin: &Path,
    args: &[String],
    alias: &str,
    st: &crate::state::Store,
) -> io::Result<Command> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("CODEX_HOME", st.session_dir(alias))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_alias_works() {
        let pattern = "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$";
        for (alias, ok) in [
            ("a1", true),
            ("abc.DEF-1", true),
            ("", false),
            ("-bad", false),
            ("../../x", false),
        ] {
            let err = super::validate_alias(alias, pattern);
            assert_eq!(err.is_ok(), ok, "alias={}", alias);
        }
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(super::shell_quote("abc"), "abc");
        assert_eq!(super::shell_quote("a b"), "'a b'");
        assert_eq!(super::shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
