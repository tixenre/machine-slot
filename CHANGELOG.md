# Changelog

## 0.2.0

First public release.

- `extend` says why it can't (no such job, still waiting, no limit); the menu bar writes long times as the CLI does (1h05m). Repository: github.com/tixenre/machine-slot.
- Long jobs: `--expect T` wins over history; a warning before a time limit (output, `status`, menu bar notification); `machine-slot extend LABEL [--by T]` and 30 More Minutes in the menu; a default limit gives a job that still works extra time twice.
- Menu bar: each job has its own «…» menu: Start Now or Remove from Queue while it waits, Stop while it runs (each asks first).
- Menu bar: SF Symbols, owner on its own line, shorter times, no empty gaps, blue bar when measured; usage in cores («3.5 of 10 cores»), in `status` too.
- Time a solo lends with `yield` no longer counts in its run time or against its limit.
- `solo` and `shared` turns: one FIFO queue for work that must run alone, side by side for the rest.
- While a solo waits, shared work starts only if its usual time fits before the running work ends.
- `--needs cpu,disk,ram,gpu,screen`: a solo waits for a quiet machine in what it needs.
- Every run is recorded: wait, wall time, exit, CPU, peak memory of the whole process tree, load, whether it ran clean, and the test's identity, which stays the same across labels and worktrees.
- `status`, `times`, `repeats`, `estimate`, `log`, `state [--json]`, `paths`.
- A read-only MCP server (`machine-slot mcp`).
- Claude Code integration: a hook, a skill and an installer.
- Progress in %: exact when the command writes it to `MACHINE_SLOT_PROGRESS` («37», «0.37», «120/300»), estimated from the usual time otherwise.
- Runs of a second or less (nothing to do) don't count toward a task's usual time.
- Live resources per running job — CPU %, memory and disk MB/s of its whole tree — in `status`, `state`, the MCP and the menu bar; each run records its peak CPU and disk MB.
- `solo --ask`: asks before quitting busy apps for a run and reopens them afterwards (`reopen` finishes the job if machine-slot was killed).
- Time limits: `--timeout T`, and a solo stops at 3 × its usual time by default (exit 124, recorded); jobs that do nothing for 5 minutes are shown as `stalled?`.
- `machine-slot yield` lends a solo's machine between two parts; `machine-slot go` (and Start Now in the menu bar) forces a waiting job; `--expect T` stands in for history, and a first-run solo stops at 30 min; `status` says where each job comes from.
- `--at HH:MM` joins the queue later, holding nothing until then.
- Loaded runs are scaled to a free machine by their load, and shared estimates to the load now; waiting shared jobs, and `estimate`, say when they should start; a solo waiting for quiet says what it waits on.
- `cancel LABEL` and `wait LABEL`; waiting solos say when they should start.
- The hook is `machine-slot hook` (milliseconds, no jq) and knows the common build tools; `hook_heavy` / `hook_timed` add more.
- Linux support (`/proc`, PSI, diskstats).
- A macOS menu bar app with a progress bar per job, live resources, start times, and opt-in Open at Login and notifications. It reads the queue only when it changes.
