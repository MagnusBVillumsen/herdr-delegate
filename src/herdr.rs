use std::fmt;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The `herdr` CLI, optionally aimed at a saved SSH machine.
#[derive(Clone, Debug)]
pub struct Herdr {
    bin: String,
    machine: Option<String>,
}

/// An error Herdr reported as `{"error": {"code", "message"}}`.
#[derive(Debug)]
pub struct HerdrError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "herdr: {} ({})", self.message, self.code)
    }
}

impl std::error::Error for HerdrError {}

/// The Herdr error code behind `err`, if it came from Herdr.
pub fn code(err: &anyhow::Error) -> Option<&str> {
    err.downcast_ref::<HerdrError>().map(|x| x.code.as_str())
}

impl Herdr {
    pub fn new(machine: Option<String>) -> Self {
        let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
        Self { bin, machine }
    }

    pub fn machine(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    fn run(&self, args: &[&str]) -> Result<(bool, String, String)> {
        let mut cmd = Command::new(&self.bin);
        if let Some(m) = &self.machine {
            cmd.args(["--machine", m]);
        }
        let out = cmd
            .args(args)
            .output()
            .with_context(|| format!("running {}", self.bin))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// Runs a JSON command and returns its `result`.
    pub fn call(&self, args: &[&str]) -> Result<Value> {
        let (ok, stdout, stderr) = self.run(args)?;
        for text in [&stdout, &stderr] {
            if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
                if let Some(e) = v.get("error") {
                    return Err(HerdrError {
                        code: e["code"].as_str().unwrap_or("unknown").into(),
                        message: e["message"].as_str().unwrap_or_default().into(),
                    }
                    .into());
                }
                if ok && text == &stdout {
                    return Ok(v.get("result").cloned().unwrap_or(v));
                }
            }
        }
        if !ok {
            bail!("herdr {}: {}", args.join(" "), stderr.trim());
        }
        bail!(
            "herdr {}: unexpected output: {}",
            args.join(" "),
            stdout.trim()
        )
    }

    /// Runs a command that prints plain text, such as `agent read`.
    pub fn text(&self, args: &[&str]) -> Result<String> {
        let (ok, stdout, stderr) = self.run(args)?;
        if !ok {
            if let Ok(v) = serde_json::from_str::<Value>(stderr.trim())
                && let Some(e) = v.get("error")
            {
                return Err(HerdrError {
                    code: e["code"].as_str().unwrap_or("unknown").into(),
                    message: e["message"].as_str().unwrap_or_default().into(),
                }
                .into());
            }
            bail!("herdr {}: {}", args.join(" "), stderr.trim());
        }
        Ok(stdout)
    }

    /// The SSH target behind the selected machine, from the local profiles.
    pub fn ssh_target(&self) -> Result<Option<String>> {
        let Some(m) = &self.machine else {
            return Ok(None);
        };
        let local = Herdr::new(None);
        let (ok, stdout, stderr) = local.run(&["machine", "list", "--json"])?;
        if !ok {
            bail!("herdr machine list: {}", stderr.trim());
        }
        let list: Vec<Value> = serde_json::from_str(&stdout).context("parsing machine list")?;
        list.iter()
            .find(|x| x["label"] == m.as_str() || x["id"] == m.as_str())
            .and_then(|x| x["target"].as_str())
            .map(|x| Some(x.to_owned()))
            .with_context(|| format!("no saved Herdr machine named {m}"))
    }
}
