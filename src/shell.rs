use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs a `sh` script with positional arguments, locally or over SSH.
pub fn run(target: Option<&str>, script: &str, args: &[&str]) -> Result<String> {
    let mut cmd = match target {
        None => {
            let mut c = Command::new("sh");
            c.args(["-c", script, "sh"]).args(args);
            c
        }
        Some(t) => {
            let mut line = format!("sh -c {} sh", quote(script));
            for a in args {
                line.push(' ');
                line.push_str(&quote(a));
            }
            let mut c = Command::new("ssh");
            c.args(["-o", "BatchMode=yes", t, &line]);
            c
        }
    };
    let out = cmd.output().context("running sh")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_single_quotes() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(run(None, r#"printf %s "$1""#, &["a b'c"]).unwrap(), "a b'c");
    }
}
