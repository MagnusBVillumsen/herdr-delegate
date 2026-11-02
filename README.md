# herdr-delegate

Let one coding agent hand a task to another in a [Herdr](https://herdr.dev)
pane and get its answer back, like a subagent you can watch.

```bash
herdr-delegate start --wait "Review the uncommitted diff for bugs"
```

The command splits the caller's pane (or opens a tab when the tab is already
split), starts Codex, Claude Code or pi there, gives it the task, and prints the
agent's final answer. You see the child work and can take over at any time.

The design borrows from [T3 Code](https://github.com/pingdotgg/t3code)'s
`delegate_task`. The child gets only the task, not the caller's history, so the
caller writes a proper brief. A child never asks for approvals it was not given:
read-only children fail an action instead of blocking on a dialog.

## Install

```bash
cargo install --git https://github.com/MagnusBVillumsen/herdr-delegate
# or
nix profile install github:MagnusBVillumsen/herdr-delegate
```

Then give your agents the skill:

```bash
for d in ~/.claude/skills ~/.agents/skills ~/.pi/agent/skills; do
  mkdir -p "$d/herdr-delegate" && herdr-delegate skill > "$d/herdr-delegate/SKILL.md"
done
```

Requires Herdr 0.9.5 or newer and the agents you delegate to on `PATH`.

## Commands

| Command | |
| --- | --- |
| `start [TASK]` | Start an agent and give it a task (stdin when TASK is `-` or missing). |
| `wait ID` | Wait for the current turn and print the answer. |
| `result ID` | Print the last answer without waiting. |
| `send ID [MESSAGE]` | Send a follow-up. |
| `status [ID]` | List tasks and their state (alias `list`). |
| `stop ID` | Interrupt the current turn. |
| `close ID` | Close the pane; `--remove-worktree` also removes its worktree. |
| `skill` | Print the agent skill. |

`start` options:

| Option | |
| --- | --- |
| `-a, --agent codex\|claude\|pi` | Default `codex`. |
| `-m, --model MODEL` | Passed to the agent. |
| `--mode read-only\|edit` | Default read-only, or edit with `--worktree`. |
| `--worktree [BASE]` | A new git worktree on branch `delegate/<id>`, from BASE (default `HEAD`). |
| `--cwd DIR` | Default: the current directory. |
| `--machine NAME` | Run on a saved Herdr machine. |
| `--placement auto\|split\|tab` | `auto` splits the caller when it is alone in its tab. |
| `--name NAME` | The agent name and task id. |
| `--trust` | Answer the agent's folder-trust dialog with yes. |
| `--wait`, `--timeout SECONDS`, `--json` | |

Exit status: 0 answer printed, 1 error, 2 the agent is waiting for input,
3 timed out.

## How it works

| | Read-only | Edit |
| --- | --- | --- |
| Codex | `-s read-only -a never` | `-s workspace-write -a never` |
| Claude Code | `--permission-mode dontAsk` with read and `git` read commands allowed | `--permission-mode auto` |
| pi | `--tools read,grep,find,ls` | default tools |

The answer comes from the agent's own session log: Codex's `task_complete`
event, or the text Claude Code and pi wrote after their last tool call. The
screen is the fallback. Tasks are recorded in
`$XDG_STATE_HOME/herdr-delegate/tasks`, and worktrees are created in
`$XDG_STATE_HOME/herdr-delegate/worktrees`.

Delegated panes are launched with `HERDR_SOUND=off`, so they do not ping you
when they finish; the agent that asked follows them instead. This needs a
Herdr that honours the variable; others ignore it.

Codex's sandbox keeps a worktree's git metadata read-only, so editing children
leave their changes uncommitted. The caller reviews the diff and commits.

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)), at your option.
