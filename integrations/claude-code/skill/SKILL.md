---
name: machine-slot
description: Use when running builds, tests, benches, baselines or UI tests on this machine, when the user says "use machine-slot", or asks how long a task takes or what is using the machine. Heavy commands go through `machine-slot shared|solo`.
---

# machine-slot

This machine has one queue for the heavy work of every session. Nobody orchestrates it: each session uses it directly.

## Do

- **Builds and tests:** `machine-slot shared "<project>: <task>" -- <command>`. They run alongside other shared work, but never during a solo.
- **Anything timed or measured** (bench, baseline, UI test, perf comparison): `machine-slot solo [--needs cpu,disk,ram,gpu,screen] "<project>: <what>" -- <command>`. It waits for its turn in one FIFO queue, then for running work to end, then for the machine to be quiet in what it needs. The default is `cpu,disk,ram`. Add `gpu` for GPU work, and `screen` for anything that drives the UI: that waits for an awake, unlocked screen and for nobody using the mouse or keyboard.
- **Labels:** use the same label for the same kind of task. Commit ids and numbers are ignored. The same command in the same repository is the same test whatever its label.
- **Waiting:** a run may wait. Run it in the background and keep working.
- **Quitting busy apps:** `solo --ask` asks the user in a dialog before quitting busy apps for the run, and reopens them afterwards. Use it only when the user wants a clean measurement and agrees to be asked; never quit apps yourself.
- **Time limits:** a solo stops at 3 × its usual time by default; give a known-long or hang-prone run `--timeout 30m` (or `0` for none). Exit 124 means it timed out.
- **Later:** `--at 23:30` queues a long solo for later without blocking anyone now.
- **Long solos are shared time.** Everything else waits while a solo runs. For a bench or baseline that takes minutes:
  - **say how long it takes** whenever it may pass 10 min, or the bench changed since its last runs: `--expect 25m`. It wins over history, and the limit becomes 3 × that. Without it a solo stops at 3 × its usual time (10 min at least; 30 min on a first run), and only gets extra time if it's still working;
  - near the cut, machine-slot warns in the job's output; `machine-slot extend "<label>" [--by 30m]` gives it more;
  - report progress from the script: `echo "3/8" > "$MACHINE_SLOT_PROGRESS"` after each pass;
  - between independent passes, call `machine-slot yield` (default 3 min, `--for 5m`): waiting builds that fit run, the machine calms down again, and your script goes on — your measurements stay clean.
- **Forcing:** `machine-slot go "<label>"` starts a waiting job now. Only when the user asks: it spoils a running solo's measurement (recorded as not clean).
- **Progress:** a long command may write its progress to the file in `$MACHINE_SLOT_PROGRESS` («37», «0.37», «120/300») for an exact bar; otherwise it's estimated from the usual time.

## Look before you run

- `machine-slot status`: what runs, what waits, % done, and the CPU / memory / disk each running job uses.
- `machine-slot estimate "<label>" [solo|shared]`: the usual time and how long it would wait now.
- `machine-slot times`, `machine-slot repeats`, `machine-slot log 20`.
- `machine-slot wait "<label>"` waits for a job and exits as it did; `machine-slot cancel "<label>"` stops your own job cleanly (never someone else's).

## Rules

- **No solo inside a shared:** it is refused, because it would wait on itself.
- **Leave other people's work alone:** never kill another session's job, and never pause or close the user's apps to make the machine quiet. If a solo starts "loaded", report what kept the machine busy.
