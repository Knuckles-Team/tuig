# Native features

`--prompt TEXT` submits one text task. `--observe-task TASK_ID` observes without
remote cancellation. Add `--headless` to print unary snapshots without opening
a terminal. These commands share the native Arbora client.

In the terminal, F6 changes theme; `l` opens logs; `i` opens the run inspector;
`r` refreshes; `c` cancels only the local request; `q` or Ctrl+C quits. `x`
requests remote cancellation only after confirmation and is disabled for observers.
Arrow, page and mouse-wheel controls scroll. Tab selects public details and
Enter or Space expands them.

With no prompt or selected task, `tuig` opens a one-task text composer. Enter
submits; Ctrl+J adds a newline; bracketed paste preserves line breaks. Escape
reviews status and `p` returns to the draft before submission. Normal letters,
including `q`, remain text while composing; Ctrl+C quits. Input is bounded to
65536 bytes. After dispatch, use the task controls above. The composer never resubmits or
starts another turn, even if the submission outcome is unknown.

There is no Python slash-command fallback. Streaming, approval attachment,
conversation continuation and durable resume remain open requirements.

## One-shot tool tasks

`tuig --one-shot` reads one JSON object from standard input. It needs `operation`
and `parameters`. It may carry `idempotency_key` and `preapproval_ref`. Tuig
submits one governed tool task through Arbora and prints its lifecycle, output
and evidence. It never sends a retry. Ctrl+C detaches and does not cancel the task.

Tuig refuses before submission when the Agent Card lacks the tool-task extension.
A preapproval counts only when Arbora attests it for the same task and it has not
expired. Without that attestation, the permission prompt stays in force and an
input-required task ends the command with an error. The `--override` flag grants
nothing here and cannot be combined with `--one-shot`.

## Local text sessions

`tuig --local-runtime --local-model PROVIDER:MODEL` opens a local ACP session.
Compose a turn with Enter. After acknowledgement, `p` opens the next draft;
`c` cancels an active turn. The backend owns completed-text history within the
process. Quitting closes that session. In an interactive session the backend may
ask the terminal to read a file, write a file or run a command inside the
workspace root. A read inside the root is served without a prompt. A write shows
its diff and a command shows its command line and directory, and each runs only
after `y`; `n` or Escape denies it. A path outside the root is refused with
`PATH_OUTSIDE_ROOT`. No central task inspector, permission grants or durable
resume are available in this path. See [configuration](configuration.md).
