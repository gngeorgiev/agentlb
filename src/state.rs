use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub alias: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub last_used_at: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recent_assignments: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalState {
    pub round_robin_index: usize,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub last_alias: String,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub sessions_dir: PathBuf,
    pub state_dir: PathBuf,
    pub session_state_dir: PathBuf,
    pub global_state_path: PathBuf,
    pub supervisor_pid_path: PathBuf,
    pub status_path: PathBuf,
    pub lock_path: PathBuf,
}

pub struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, perms)
}

impl Store {
    pub fn new() -> io::Result<Self> {
        let root = crate::config::agentlb_root()?;
        let config_path = crate::config::config_path()?;
        Ok(Self {
            root: root.clone(),
            config_path,
            sessions_dir: root.join("sessions"),
            state_dir: root.join("state"),
            session_state_dir: root.join("state").join("sessions"),
            global_state_path: root.join("state").join("global.json"),
            supervisor_pid_path: root.join("supervisor.pid"),
            status_path: root.join("status.json"),
            lock_path: root.join("locks").join("state.lock"),
        })
    }

    pub fn ensure_layout(&self) -> io::Result<()> {
        let dirs = [
            self.root.as_path(),
            self.sessions_dir.as_path(),
            self.state_dir.as_path(),
            self.session_state_dir.as_path(),
            self.lock_path
                .parent()
                .ok_or_else(|| io::Error::other("invalid lock path"))?,
        ];
        for dir in dirs {
            fs::create_dir_all(dir)?;
            set_mode(dir, 0o700)?;
        }

        let _f = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_path)?;
        set_mode(&self.lock_path, 0o600)
    }

    pub fn lock(&self) -> io::Result<LockGuard> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_path)?;
        file.lock_exclusive()?;
        Ok(LockGuard { file })
    }

    pub fn load_global(&self) -> io::Result<GlobalState> {
        match fs::read(&self.global_state_path) {
            Ok(bytes) => serde_json::from_slice::<GlobalState>(&bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid global state: {}", err),
                )
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(GlobalState::default()),
            Err(err) => Err(err),
        }
    }

    pub fn save_global(&self, g: &GlobalState) -> io::Result<()> {
        self.write_json(&self.global_state_path, g)
    }

    pub fn session_dir(&self, alias: &str) -> PathBuf {
        self.sessions_dir.join(alias)
    }

    pub fn session_state_path(&self, alias: &str) -> PathBuf {
        self.session_state_dir.join(format!("{}.json", alias))
    }

    pub fn list_aliases(&self) -> io::Result<Vec<String>> {
        let mut aliases = vec![];
        match fs::read_dir(&self.sessions_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        let alias = entry.file_name().to_string_lossy().to_string();
                        if alias.starts_with('.') {
                            continue;
                        }
                        aliases.push(alias);
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(err) => return Err(err),
        }
        aliases.sort();
        Ok(aliases)
    }

    pub fn load_session(&self, alias: &str) -> io::Result<SessionState> {
        let path = self.session_state_path(alias);
        match fs::read(path) {
            Ok(bytes) => {
                let mut ss = serde_json::from_slice::<SessionState>(&bytes).map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid session state: {}", err),
                    )
                })?;
                if ss.alias.is_empty() {
                    ss.alias = alias.to_string();
                }
                if ss.created_at.is_empty() {
                    ss.created_at = now_rfc3339();
                }
                Ok(ss)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(SessionState {
                alias: alias.to_string(),
                created_at: now_rfc3339(),
                ..SessionState::default()
            }),
            Err(err) => Err(err),
        }
    }

    pub fn save_session(&self, ss: &SessionState) -> io::Result<()> {
        if ss.alias.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty alias"));
        }
        let mut to_save = ss.clone();
        if to_save.created_at.is_empty() {
            to_save.created_at = now_rfc3339();
        }
        self.write_json(&self.session_state_path(&to_save.alias), &to_save)
    }

    pub fn record_assignment(&self, alias: &str, spread_window: usize) -> io::Result<()> {
        let mut ss = self.load_session(alias)?;
        let now = Utc::now();
        ss.last_used_at = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        ss.recent_assignments.push(now.timestamp());
        if spread_window > 0 && ss.recent_assignments.len() > spread_window {
            let start = ss.recent_assignments.len() - spread_window;
            ss.recent_assignments = ss.recent_assignments[start..].to_vec();
        }
        self.save_session(&ss)
    }

    fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> io::Result<()> {
        self.write_json_atomic(path, value)
    }

    pub fn write_json_atomic<T: Serialize>(&self, path: &Path, value: &T) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid path"))?;
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|err| io::Error::other(format!("serialize json failed: {}", err)))?;
        bytes.push(b'\n');
        let tmp = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("state"),
            std::process::id()
        ));
        fs::write(&tmp, bytes)?;
        set_mode(&tmp, 0o600)?;
        fs::rename(&tmp, path)?;
        set_mode(path, 0o600)
    }

    pub fn write_text_atomic(&self, path: &Path, value: &str, mode: u32) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid path"))?;
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
        let tmp = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("state"),
            std::process::id()
        ));
        fs::write(&tmp, value.as_bytes())?;
        set_mode(&tmp, mode)?;
        fs::rename(&tmp, path)?;
        set_mode(path, mode)
    }
}
