use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::agent::{Kind, Mode};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
}

/// A delegated task, kept so later commands can find its agent again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub agent: Kind,
    pub mode: Mode,
    pub machine: Option<String>,
    pub pane: String,
    pub cwd: String,
    pub worktree: Option<Worktree>,
    pub session: Option<String>,
    pub created: u64,
    /// The brief, while the agent waits for someone to trust its folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/state")
        }
    };
    Ok(base.join("herdr-delegate/tasks"))
}

impl Task {
    pub fn save(&self) -> Result<()> {
        let dir = dir()?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.id));
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn load(id: &str) -> Result<Self> {
        let path = dir()?.join(format!("{id}.json"));
        let data = std::fs::read(&path).with_context(|| format!("no delegated task named {id}"))?;
        serde_json::from_slice(&data).with_context(|| format!("reading {}", path.display()))
    }

    pub fn remove(&self) -> Result<()> {
        let path = dir()?.join(format!("{}.json", self.id));
        std::fs::remove_file(path).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e.into()),
        })
    }

    pub fn list() -> Result<Vec<Self>> {
        let dir = dir()?;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut tasks = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|d| serde_json::from_slice::<Task>(&d).ok())
            .collect::<Vec<_>>();
        tasks.sort_by_key(|t| t.created);
        Ok(tasks)
    }
}

/// 16 random bytes from the OS, for ids that must not collide.
pub fn random_bytes() -> Result<[u8; 16]> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

pub fn uuid_v4() -> Result<String> {
    let mut b = random_bytes()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}
