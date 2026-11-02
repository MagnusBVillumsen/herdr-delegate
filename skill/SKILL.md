---
name: herdr-delegate
description: Hand a task to another coding agent (Codex, Claude Code or pi) in a Herdr pane next to you and get its answer back, like a subagent you and the user can watch. Use for a second opinion or review from a different model ("ask Codex to review this"), for independent side tasks that can run while you work, or when the user asks you to delegate to, prompt or consult another agent. Requires running inside Herdr (HERDR_ENV=1) or a saved Herdr machine.
---

# herdr-delegate

`herdr-delegate` starts another agent in a Herdr pane beside yours, gives it one
task, and returns its final answer. The user sees the agent work in the pane and
can take over at any time.

The child gets **only the task you write**, none of your history. Write a brief
a capable colleague could act on cold: the goal, the files or diff to look at,
what you have already ruled out, and what form the answer should take.

## Start, wait, read

```bash
# Read-only review by Codex (the default agent); waits and prints the answer
herdr-delegate start --wait "Review the uncommitted diff in this repo for bugs. \
List each finding as file:line, what breaks, and how sure you are."

# Start in the background, keep working, collect later
id=$(herdr-delegate start --json -a claude "..." | jq -r .id)
herdr-delegate wait "$id"          # blocks until the turn ends, prints the answer
herdr-delegate send "$id" "Follow-up question" --wait
herdr-delegate status              # all delegated tasks and their state
herdr-delegate close "$id"         # close its pane when you are done with it
```

- `-a codex|claude|pi` picks the agent; `-m` the model.
- The default is **read-only**: it can read and run read-only commands, nothing
  else.
- `--worktree [BASE]` gives the child its own git worktree on branch
  `delegate/<id>` and lets it edit. It leaves its changes uncommitted: read them
  with `git -C <worktree> diff`, then commit or discard them yourself.
  `close <id> --remove-worktree` removes the worktree once its changes are
  committed or discarded.
- `--machine <name>` runs the child on a saved Herdr machine (`herdr machine list`).
- Use `--json` when you need the pane, worktree path or status as data.

## Exit status

| Code | Meaning |
| --- | --- |
| 0 | The answer is on stdout. |
| 1 | Error, or the agent left no answer. |
| 2 | The agent is waiting for input (an approval or question). The message shows its screen. |
| 3 | Timed out (`--timeout SECONDS`); the agent keeps working. Run `wait` again later. |

On exit 2, decide whether the choice is yours to make. If it is not, ask the
user. A **folder-trust** question is always the user's: never pass `--trust`
or answer it yourself without their OK. Once the user has answered it, run
`herdr-delegate send <id>` to deliver the held-back task.

## Etiquette

- Delegate work that stands on its own. Do not hand off something you need
  in the next minute and then sit idle waiting for it.
- One child per question. Close children you no longer need.
- Read the answer critically before you rely on it or pass it on. Say which
  agent it came from.
