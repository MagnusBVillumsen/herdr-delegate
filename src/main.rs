mod agent;
mod herdr;
mod shell;
mod task;

use std::fmt;
use std::io::Read;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::Value;

use agent::{Kind, Mode};
use herdr::Herdr;
use task::{Task, Worktree};

/// Hand a task to another coding agent in a Herdr pane and get its answer back.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start an agent in a new pane and give it a task
    Start(StartArgs),
    /// Wait for a task's current turn to finish and print the answer
    Wait {
        id: String,
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Print the answer of a finished turn without waiting
    Result {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Send a follow-up message to a delegated agent
    Send {
        id: String,
        /// The message, or `-` for stdin. Omitted: deliver a brief held back
        /// by a folder-trust dialog.
        message: Option<String>,
        #[arg(long)]
        wait: bool,
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// List delegated tasks and their state
    #[command(visible_alias = "list")]
    Status {
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Interrupt a delegated agent's current turn
    Stop { id: String },
    /// Close a delegated agent's pane and forget the task
    Close {
        id: String,
        /// Also remove its worktree (refused if it has uncommitted changes)
        #[arg(long)]
        remove_worktree: bool,
    },
    /// Print the agent skill that teaches this tool
    Skill,
}

#[derive(Args)]
struct StartArgs {
    /// The task, or `-` (or nothing) to read it from stdin
    task: Option<String>,
    #[arg(short, long, value_enum, default_value_t = Kind::Codex)]
    agent: Kind,
    /// Default: read-only, or edit with --worktree
    #[arg(long, value_enum)]
    mode: Option<Mode>,
    #[arg(short, long)]
    model: Option<String>,
    /// Working directory. Default: the current one
    #[arg(long)]
    cwd: Option<String>,
    /// Work in a new git worktree branched from BASE (default HEAD)
    #[arg(long, value_name = "BASE", num_args = 0..=1, default_missing_value = "HEAD")]
    worktree: Option<String>,
    /// Agent name and task id. Default: <agent>-<4 hex digits>
    #[arg(long)]
    name: Option<String>,
    /// Run on a saved Herdr machine instead of this one
    #[arg(long)]
    machine: Option<String>,
    /// Answer the agent's folder-trust dialog with yes. Only with the user's OK
    #[arg(long)]
    trust: bool,
    #[arg(long, value_enum, default_value_t = Placement::Auto)]
    placement: Placement,
    /// Wait for the answer instead of returning once the agent is working
    #[arg(long)]
    wait: bool,
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Placement {
    /// Split the calling pane when it is alone in its tab, else open a tab
    Auto,
    Split,
    Tab,
}

/// An error with its own exit status: 2 when the agent needs input, 3 on
/// timeout.
#[derive(Debug)]
struct Exit {
    code: i32,
    message: String,
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Exit {}

fn exit(code: i32, message: impl Into<String>) -> anyhow::Error {
    Exit {
        code,
        message: message.into(),
    }
    .into()
}

#[derive(Serialize)]
struct Report<'a> {
    #[serde(flatten)]
    task: &'a Task,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    screen: Option<String>,
}

fn main() {
    let code = match run(Cli::parse()) {
        Ok(()) => 0,
        Err(e) => match e.downcast_ref::<Exit>() {
            Some(x) => {
                if !x.message.is_empty() {
                    eprintln!("{}", x.message);
                }
                x.code
            }
            None => {
                eprintln!("herdr-delegate: {e:#}");
                1
            }
        },
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Start(a) => start(a),
        Command::Wait { id, timeout, json } => {
            let mut task = Task::load(&id)?;
            finish(&mut task, timeout, json)
        }
        Command::Result { id, json } => {
            let mut task = Task::load(&id)?;
            let herdr = Herdr::new(task.machine.clone());
            let status = status(&herdr, &task.id);
            if status == "working" {
                return Err(exit(3, format!("{id} is still working")));
            }
            report(&herdr, &mut task, status, json)
        }
        Command::Send {
            id,
            message,
            wait,
            timeout,
            json,
        } => {
            let mut task = Task::load(&id)?;
            let herdr = Herdr::new(task.machine.clone());
            let text = match message {
                Some(m) => read_text(Some(m))?,
                None => task
                    .pending
                    .take()
                    .with_context(|| format!("{id} has no held-back brief; give a message"))?,
            };
            submit(&herdr, &task, &text)?;
            task.save()?;
            if wait {
                finish(&mut task, timeout, json)
            } else {
                print_started(&task, json)
            }
        }
        Command::Status { id, json } => {
            let tasks = match id {
                Some(id) => vec![Task::load(&id)?],
                None => Task::list()?,
            };
            let rows = tasks
                .iter()
                .map(|t| {
                    let s = status(&Herdr::new(t.machine.clone()), &t.id);
                    Report {
                        task: t,
                        status: s,
                        answer: None,
                        screen: None,
                    }
                })
                .collect::<Vec<_>>();
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for r in rows {
                    println!(
                        "{:<14} {:<6} {:<8} {:<10} {}",
                        r.task.id,
                        r.task.agent.as_str(),
                        r.status,
                        r.task.machine.as_deref().unwrap_or("local"),
                        r.task.cwd
                    );
                }
            }
            Ok(())
        }
        Command::Stop { id } => {
            let task = Task::load(&id)?;
            Herdr::new(task.machine.clone()).call(&["agent", "send-keys", &id, "esc"])?;
            Ok(())
        }
        Command::Close {
            id,
            remove_worktree,
        } => close(&id, remove_worktree),
        Command::Skill => {
            print!("{}", include_str!("../skill/SKILL.md"));
            Ok(())
        }
    }
}

fn read_text(arg: Option<String>) -> Result<String> {
    let text = match arg.as_deref() {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
        Some(x) => x.to_owned(),
    };
    if text.trim().is_empty() {
        bail!("the task is empty");
    }
    Ok(text)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn start(a: StartArgs) -> Result<()> {
    let text = read_text(a.task)?;
    let herdr = Herdr::new(a.machine.clone());
    let target = herdr.ssh_target()?;
    let id = match a.name {
        Some(n) if valid_name(&n) => n,
        Some(n) => bail!("{n}: names are a lowercase letter, then up to 31 of a-z 0-9 - _"),
        None => {
            let b = task::random_bytes()?;
            format!("{}-{:02x}{:02x}", a.agent.as_str(), b[0], b[1])
        }
    };
    if herdr.call(&["agent", "get", &id]).is_ok() {
        bail!("an agent named {id} is already running");
    }
    let mut cwd = match a.cwd {
        Some(c) => c,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    let worktree = match &a.worktree {
        Some(base) => Some(add_worktree(target.as_deref(), &cwd, &id, base)?),
        None => None,
    };
    if let Some(w) = &worktree {
        cwd = w.path.clone();
    }
    let mode = a.mode.unwrap_or(match worktree {
        Some(_) => Mode::Edit,
        None => Mode::ReadOnly,
    });
    let pane = place(&herdr, &cwd, &id, a.placement)?;
    let session = match a.agent.presets_session() {
        true => Some(task::uuid_v4()?),
        false => None,
    };
    let mut task = Task {
        id: id.clone(),
        agent: a.agent,
        mode,
        machine: a.machine,
        pane: pane.clone(),
        cwd,
        worktree,
        session,
        created: task::now(),
        pending: None,
    };
    task.save()?;

    let extra = a
        .agent
        .args(mode, a.model.as_deref(), task.session.as_deref());
    let mut args = vec![
        "agent",
        "start",
        &id,
        "--kind",
        a.agent.as_str(),
        "--pane",
        &pane,
        "--timeout",
        "60000",
        "--",
    ];
    args.extend(extra.iter().map(String::as_str));
    let brief = brief(&task, &text);
    if let Err(e) = herdr.call(&args) {
        if herdr::code(&e) != Some("agent_not_ready") {
            return Err(e);
        }
        let screen = screen(&herdr, &id);
        if !a.agent.is_trust_dialog(&screen) {
            return Err(exit(2, blocked_message(&task, &screen)));
        }
        if !(a.trust || task.worktree.is_some()) {
            task.pending = Some(brief);
            task.save()?;
            return Err(exit(
                2,
                format!(
                    "{id} is asking whether to trust {} (pane {pane}). Ask the user. \
                     Once they have answered it, run `herdr-delegate send {id}` to deliver the task, \
                     or `herdr-delegate close {id}`.",
                    task.cwd
                ),
            ));
        }
        herdr.call(&["agent", "send-keys", &id, "enter"])?;
        herdr.call(&[
            "agent",
            "wait",
            &id,
            "--until",
            "idle",
            "--timeout",
            "60000",
        ])?;
    }
    submit(&herdr, &task, &brief)?;
    if a.wait {
        finish(&mut task, a.timeout, a.json)
    } else {
        print_started(&task, a.json)
    }
}

fn add_worktree(target: Option<&str>, cwd: &str, id: &str, base: &str) -> Result<Worktree> {
    let script = r#"cd "$1" && root=$(git rev-parse --show-toplevel) &&
dir="${XDG_STATE_HOME:-$HOME/.local/state}/herdr-delegate/worktrees/$(basename "$root")-$2" &&
git worktree add -q -b "delegate/$2" "$dir" "$3" >&2 && printf %s "$dir""#;
    let path = shell::run(target, script, &[cwd, id, base]).context("creating worktree")?;
    Ok(Worktree {
        path,
        branch: format!("delegate/{id}"),
    })
}

/// Opens the pane the agent runs in and returns its id.
/// Herdr plays no notification sounds for panes launched with this; the
/// caller follows the child itself. Herdr without support ignores it.
const MUTE: &str = "HERDR_SOUND=off";

fn place(herdr: &Herdr, cwd: &str, id: &str, placement: Placement) -> Result<String> {
    let caller = match herdr.machine() {
        None if std::env::var("HERDR_ENV").as_deref() == Ok("1") => {
            std::env::var("HERDR_PANE_ID").ok()
        }
        _ => None,
    };
    let Some(caller) = caller else {
        let list = herdr.call(&["workspace", "list"])?;
        let existing = list["workspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|w| w["label"] == "delegate")
            .and_then(|w| w["workspace_id"].as_str());
        let r = match existing {
            Some(ws) => herdr.call(&[
                "tab",
                "create",
                "--workspace",
                ws,
                "--cwd",
                cwd,
                "--label",
                id,
                "--no-focus",
                "--env",
                MUTE,
            ])?,
            None => herdr.call(&[
                "workspace",
                "create",
                "--cwd",
                cwd,
                "--label",
                "delegate",
                "--no-focus",
                "--env",
                MUTE,
            ])?,
        };
        return string(&r["root_pane"]["pane_id"]);
    };
    let layout = herdr.call(&["pane", "layout", "--pane", &caller])?["layout"].clone();
    let panes = layout["panes"].as_array().cloned().unwrap_or_default();
    let split = match placement {
        Placement::Auto => panes.len() <= 1,
        Placement::Split => true,
        Placement::Tab => false,
    };
    if split {
        let width = panes
            .iter()
            .find(|p| p["pane_id"] == caller.as_str())
            .and_then(|p| p["rect"]["width"].as_u64())
            .unwrap_or(0);
        let direction = if width >= 100 { "right" } else { "down" };
        let r = herdr.call(&[
            "pane",
            "split",
            &caller,
            "--direction",
            direction,
            "--cwd",
            cwd,
            "--no-focus",
            "--env",
            MUTE,
        ])?;
        return string(&r["pane"]["pane_id"]);
    }
    let ws = string(&layout["workspace_id"])?;
    let r = herdr.call(&[
        "tab",
        "create",
        "--workspace",
        &ws,
        "--cwd",
        cwd,
        "--label",
        id,
        "--no-focus",
        "--env",
        MUTE,
    ])?;
    string(&r["root_pane"]["pane_id"])
}

fn string(v: &Value) -> Result<String> {
    v.as_str()
        .map(String::from)
        .with_context(|| format!("herdr returned no id: {v}"))
}

fn brief(task: &Task, text: &str) -> String {
    let scope = match (task.mode, &task.worktree) {
        (Mode::ReadOnly, _) => {
            "You are read-only: read files and run read-only commands, but do not modify anything. \
             Run commands from the working directory (`git log`, not `git -C <path> log`)."
                .to_owned()
        }
        (Mode::Edit, Some(w)) => format!(
            "You may edit files. This is your own git worktree on branch {}. Leave your changes \
             uncommitted: the agent that asked reviews the diff and commits it. Do not touch \
             other checkouts.",
            w.branch
        ),
        (Mode::Edit, None) => {
            "You may edit files in this directory. Do not commit, push or merge unless the task \
             says so."
                .to_owned()
        }
    };
    format!(
        "[Delegated task {id}] Another agent handed you this task through herdr-delegate. You \
         have none of its history: everything you need is below. Work in {cwd}. {scope}\n\n\
         When you are done, give your result as your final message. It is passed back verbatim \
         to the agent that asked, so make it complete on its own: findings, what you changed, \
         and anything left open. If you need a decision you cannot make, ask for it in your \
         final message and stop.\n\n---\n\n{text}",
        id = task.id,
        cwd = task.cwd,
    )
}

/// Submits `text` and returns once the agent has started working on it.
fn submit(herdr: &Herdr, task: &Task, text: &str) -> Result<()> {
    let r = herdr.call(&[
        "agent",
        "prompt",
        &task.id,
        text,
        "--wait",
        "--until",
        "working",
        "--until",
        "blocked",
        "--timeout",
        "30000",
    ]);
    match r {
        Ok(v) if v["agent"]["agent_status"] == "blocked" => {
            Err(exit(2, blocked_message(task, &screen(herdr, &task.id))))
        }
        Ok(_) => Ok(()),
        Err(e) if herdr::code(&e) == Some("agent_blocked") => {
            Err(exit(2, blocked_message(task, &screen(herdr, &task.id))))
        }
        Err(e) if herdr::code(&e) == Some("agent_prompt_stalled") => Err(exit(
            1,
            format!(
                "{} did not start working on the message; look at pane {} before sending it again",
                task.id, task.pane
            ),
        )),
        Err(e) => Err(e),
    }
}

fn status(herdr: &Herdr, id: &str) -> String {
    herdr
        .call(&["agent", "get", id])
        .ok()
        .and_then(|v| v["agent"]["agent_status"].as_str().map(String::from))
        .unwrap_or_else(|| "gone".into())
}

fn screen(herdr: &Herdr, id: &str) -> String {
    herdr
        .text(&["agent", "read", id, "--source", "visible"])
        .unwrap_or_default()
}

fn blocked_message(task: &Task, screen: &str) -> String {
    let tail = screen
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();
    let tail = tail[tail.len().saturating_sub(15)..].join("\n");
    format!(
        "{} is waiting for input in pane {}:\n\n{tail}\n\nAnswer with `herdr agent send-keys {} <keys>` \
         if the choice is yours to make, or ask the user.",
        task.id, task.pane, task.id
    )
}

/// Waits for the current turn to settle, then reports it.
fn finish(task: &mut Task, timeout: Option<u64>, json: bool) -> Result<()> {
    let herdr = Herdr::new(task.machine.clone());
    let ms = timeout.map(|s| (s * 1000).to_string());
    let mut args = vec![
        "agent", "wait", &task.id, "--until", "idle", "--until", "done", "--until", "blocked",
    ];
    if let Some(ms) = &ms {
        args.extend(["--timeout", ms]);
    }
    let status = match herdr.call(&args) {
        Ok(v) => v["agent"]["agent_status"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        Err(e) if herdr::code(&e) == Some("timeout") => {
            return Err(exit(
                3,
                format!(
                    "{} is still working; `herdr-delegate wait {}` to keep waiting",
                    task.id, task.id
                ),
            ));
        }
        Err(e)
            if matches!(
                herdr::code(&e),
                Some("agent_not_found" | "agent_not_running")
            ) =>
        {
            "gone".into()
        }
        Err(e) => return Err(e),
    };
    report(&herdr, task, status, json)
}

fn report(herdr: &Herdr, task: &mut Task, status: String, json: bool) -> Result<()> {
    if status == "blocked" {
        let screen = screen(herdr, &task.id);
        if json {
            print_json(task, status, None, Some(screen))?;
            return Err(exit(2, ""));
        }
        return Err(exit(2, blocked_message(task, &screen)));
    }
    let answer = answer(herdr, task)?;
    if json {
        return print_json(task, status, answer, None);
    }
    match answer {
        Some(a) => {
            println!("{}", a.trim_end());
            Ok(())
        }
        None => Err(exit(
            1,
            format!("{} left no answer (status {status})", task.id),
        )),
    }
}

/// The last turn's answer from the session log, or the screen when the log
/// has none.
fn answer(herdr: &Herdr, task: &mut Task) -> Result<Option<String>> {
    if task.session.is_none() {
        task.session = herdr.call(&["agent", "get", &task.id]).ok().and_then(|v| {
            v["agent"]["agent_session"]["value"]
                .as_str()
                .map(String::from)
        });
        task.save()?;
    }
    if let Some(session) = &task.session {
        let target = herdr.ssh_target()?;
        let mut turn = None;
        for _ in 0..20 {
            let log = shell::run(target.as_deref(), task.agent.session_script(), &[session])
                .unwrap_or_default();
            let t = task.agent.last_turn(&log);
            let done = t.done;
            turn = Some(t);
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if let Some(a) = turn.and_then(|t| t.answer) {
            return Ok(Some(a));
        }
    }
    Ok(herdr
        .text(&[
            "agent",
            "read",
            &task.id,
            "--source",
            "recent-unwrapped",
            "--lines",
            "200",
        ])
        .ok()
        .filter(|s| !s.trim().is_empty()))
}

fn print_json(
    task: &Task,
    status: String,
    answer: Option<String>,
    screen: Option<String>,
) -> Result<()> {
    let r = Report {
        task,
        status,
        answer,
        screen,
    };
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}

fn print_started(task: &Task, json: bool) -> Result<()> {
    if json {
        return print_json(task, "working".into(), None, None);
    }
    println!(
        "{} is working in pane {}{}. `herdr-delegate wait {}` prints its answer.",
        task.id,
        task.pane,
        task.machine
            .as_deref()
            .map(|m| format!(" on {m}"))
            .unwrap_or_default(),
        task.id
    );
    Ok(())
}

fn close(id: &str, remove_worktree: bool) -> Result<()> {
    let task = Task::load(id)?;
    let herdr = Herdr::new(task.machine.clone());
    let pane = herdr
        .call(&["agent", "get", id])
        .ok()
        .and_then(|v| v["agent"]["pane_id"].as_str().map(String::from))
        .unwrap_or_else(|| task.pane.clone());
    if let Err(e) = herdr.call(&["pane", "close", &pane])
        && !matches!(herdr::code(&e), Some("pane_not_found"))
    {
        return Err(e);
    }
    if let (true, Some(w)) = (remove_worktree, &task.worktree) {
        let target = herdr.ssh_target()?;
        shell::run(
            target.as_deref(),
            r#"git -C "$1" worktree remove "$1""#,
            &[&w.path],
        )
        .context("removing worktree (commit or discard its changes first)")?;
        println!("Removed {}; branch {} is kept.", w.path, w.branch);
    } else if let Some(w) = &task.worktree {
        println!("Worktree kept at {} (branch {}).", w.path, w.branch);
    }
    task.remove()
}
