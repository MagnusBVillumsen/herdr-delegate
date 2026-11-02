use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct Turn {
    pub answer: Option<String>,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Codex,
    Claude,
    Pi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Read files and run read-only commands; never edit.
    ReadOnly,
    /// Edit files in its working directory without asking.
    Edit,
}

const CLAUDE_READ_ONLY_TOOLS: &str = "Read,Grep,Glob,\
Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *),Bash(git blame *),\
Bash(git -C * diff *),Bash(git -C * log *),Bash(git -C * show *),Bash(git -C * status *),\
Bash(ls *),Bash(rg *)";

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Codex => "codex",
            Kind::Claude => "claude",
            Kind::Pi => "pi",
        }
    }

    /// Whether we choose the session id ourselves at launch. Codex has no
    /// flag for it, so Herdr reports it once the session exists.
    pub fn presets_session(self) -> bool {
        self != Kind::Codex
    }

    /// Arguments after `--` for `herdr agent start`.
    ///
    /// Read-only children never ask for approval: anything outside their
    /// allowance fails instead of leaving them blocked on a dialog.
    pub fn args(self, mode: Mode, model: Option<&str>, session: Option<&str>) -> Vec<String> {
        let mut a: Vec<String> = match (self, mode) {
            (Kind::Codex, Mode::ReadOnly) => vec!["-s", "read-only", "-a", "never"],
            (Kind::Codex, Mode::Edit) => vec!["-s", "workspace-write", "-a", "never"],
            (Kind::Claude, Mode::ReadOnly) => vec![
                "--permission-mode",
                "dontAsk",
                "--allowedTools",
                CLAUDE_READ_ONLY_TOOLS,
            ],
            (Kind::Claude, Mode::Edit) => vec!["--permission-mode", "auto"],
            (Kind::Pi, Mode::ReadOnly) => vec!["--tools", "read,grep,find,ls"],
            (Kind::Pi, Mode::Edit) => vec![],
        }
        .into_iter()
        .map(String::from)
        .collect();
        if let Some(m) = model {
            a.extend([
                if self == Kind::Codex { "-m" } else { "--model" }.into(),
                m.into(),
            ]);
        }
        if let Some(s) = session {
            a.extend(["--session-id".into(), s.into()]);
        }
        a
    }

    /// Whether `screen` shows the agent's folder-trust dialog.
    pub fn is_trust_dialog(self, screen: &str) -> bool {
        let s = screen.to_lowercase();
        s.contains("trust this folder") || s.contains("trust the files")
    }

    /// A `sh` script that prints the session log for the id in `$1`.
    pub fn session_script(self) -> &'static str {
        match self {
            Kind::Codex => {
                r#"f=$(find "${CODEX_HOME:-$HOME/.codex}/sessions" -name "rollout-*-$1.jsonl" 2>/dev/null | head -n 1); [ -n "$f" ] && cat "$f""#
            }
            Kind::Claude => {
                r#"f=$(find "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects" -maxdepth 2 -name "$1.jsonl" 2>/dev/null | head -n 1); [ -n "$f" ] && cat "$f""#
            }
            Kind::Pi => {
                r#"f=$(find "${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/sessions" -name "*$1*.jsonl" 2>/dev/null | head -n 1); [ -n "$f" ] && cat "$f""#
            }
        }
    }

    /// The last turn in a session log. Its answer is the text the agent
    /// wrote after its last tool call; the log may record the end of a turn
    /// a moment after Herdr sees the agent go idle.
    pub fn last_turn(self, log: &str) -> Turn {
        let lines = log
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok());
        let mut answer = Vec::new();
        let mut done = false;
        match self {
            Kind::Codex => {
                for v in lines.filter(|v| v["type"] == "event_msg") {
                    match v["payload"]["type"].as_str() {
                        Some("task_started") => done = false,
                        Some("task_complete") => {
                            done = true;
                            answer = v["payload"]["last_agent_message"]
                                .as_str()
                                .map(String::from)
                                .into_iter()
                                .collect();
                        }
                        _ => {}
                    }
                }
            }
            Kind::Claude => {
                for v in lines {
                    let content = &v["message"]["content"];
                    match v["type"].as_str() {
                        Some("user") if v["isMeta"] != true => {
                            if content.is_string() || blocks(content).any(|b| b["type"] == "text") {
                                answer.clear();
                            }
                            done = false;
                        }
                        Some("assistant") => {
                            collect(content, "tool_use", &mut answer);
                            done = false;
                        }
                        Some("system") if v["subtype"] == "turn_duration" => done = true,
                        _ => {}
                    }
                }
            }
            Kind::Pi => {
                for v in lines.filter(|v| v["type"] == "message") {
                    let content = &v["message"]["content"];
                    match v["message"]["role"].as_str() {
                        Some("user") => {
                            answer.clear();
                            done = false;
                        }
                        Some("assistant") => {
                            collect(content, "toolCall", &mut answer);
                            done = v["message"]["stopReason"]
                                .as_str()
                                .is_some_and(|r| r != "toolUse");
                        }
                        _ => done = false,
                    }
                }
            }
        }
        let text = answer.join("\n\n");
        Turn {
            answer: (!text.trim().is_empty()).then_some(text),
            done,
        }
    }
}

fn blocks(content: &Value) -> impl Iterator<Item = &Value> {
    content.as_array().into_iter().flatten()
}

fn collect(content: &Value, tool: &str, answer: &mut Vec<String>) {
    for b in blocks(content) {
        if b["type"] == tool {
            answer.clear();
        } else if b["type"] == "text"
            && let Some(t) = b["text"].as_str()
        {
            answer.push(t.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_answer_is_last_task_complete() {
        let log = r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"one"}}
{"type":"event_msg","payload":{"type":"task_started"}}
{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"two"}}"#;
        let t = Kind::Codex.last_turn(log);
        assert_eq!((t.answer.as_deref(), t.done), (Some("two"), true));
        let running = format!(
            "{log}\n{}",
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#
        );
        assert!(!Kind::Codex.last_turn(&running).done);
    }

    #[test]
    fn claude_answer_skips_narration_before_tools() {
        let log = r#"{"type":"user","message":{"content":"old"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"old answer"}]}}
{"type":"user","message":{"content":[{"type":"text","text":"review"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Let me look"},{"type":"tool_use"}]}}
{"type":"user","message":{"content":[{"type":"tool_result"}]}}
{"type":"user","isMeta":true,"message":{"content":"reminder"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Looks good"}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Ship it"}]}}"#;
        let t = Kind::Claude.last_turn(log);
        assert_eq!(t.answer.as_deref(), Some("Looks good\n\nShip it"));
        assert!(!t.done);
        let ended = format!(
            "{log}\n{}",
            r#"{"type":"system","subtype":"turn_duration"}"#
        );
        assert!(Kind::Claude.last_turn(&ended).done);
    }

    #[test]
    fn pi_answer_follows_last_tool_call() {
        let log = r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"q"}]}}
{"type":"message","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking"},{"type":"text","text":"checking"},{"type":"toolCall"}]}}
{"type":"message","message":{"role":"toolResult","content":[{"type":"text","text":"output"}]}}
{"type":"message","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"done"}]}}"#;
        let t = Kind::Pi.last_turn(log);
        assert_eq!((t.answer.as_deref(), t.done), (Some("done"), true));
    }
}
