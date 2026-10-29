# machine-slot

**A machine-wide queue for heavy work.** Builds and tests run side by side. Benchmarks, baselines and UI tests run alone, on a quiet machine. Every run is timed, so the queue learns how long things take.

It's built for a laptop shared by many processes at once: several terminals, several AI coding agents, a watcher or two. Without coordination, a benchmark can run next to a release build and measure nothing.

```console
$ machine-slot shared "api: tests" -- cargo test -p api
$ machine-slot solo --needs cpu,disk,ram,gpu "renderer: baseline" -- ./bench.sh
machine-slot: solo «renderer: baseline» waits (0s):
  shared: api: tests (41s of ~1:10, 29s left)
machine-slot: solo «renderer: baseline» waits for a quiet Mac: cpu 34 %, gpu 22 %
machine-slot: solo «renderer: baseline»: the Mac is quiet after 18s
```

macOS (Apple silicon and Intel) and Linux. No daemon, no dependencies.

## Install

```bash
cargo install machine-slot                       # from crates.io
brew install mancino-dev/tap/machine-slot        # Homebrew, with the menu bar app on macOS
```

Or take a binary from the [releases](https://github.com/mancino-dev/machine-slot/releases).

Optional:

```bash
git clone https://github.com/mancino-dev/machine-slot && cd machine-slot
integrations/claude-code/install.sh     # Claude Code hook, skill and MCP server
macos/menubar/build.sh --install        # the menu bar app; "Open at Login" in its menu
```

## Two kinds of turns

**`shared`** is for work whose speed doesn't matter much: builds, test suites, linters. Shared jobs run together, and never while a solo runs.

**`solo`** is for anything you time or measure. It takes its place in one FIFO queue. When its turn comes, it waits for the running shared work to end, and from then on nothing new starts. Then it waits for the machine to be quiet in what it needs, for 5 seconds in a row:

| need | quiet means |
|---|---|
| `cpu` | the whole machine under 15 % busy |
| `disk` | under 5 MB/s read + write |
| `ram` | the kernel's memory pressure is normal |
| `gpu` | GPU utilization under 10 % |
| `screen` | display awake, session unlocked, no mouse or keyboard input for 10 s |

The default is `cpu,disk,ram`. Add `gpu` for GPU work and `screen` for UI tests, which also keeps a UI test from fighting you for the mouse. If the machine doesn't calm down within 60 s (10 min with `screen`), the solo starts anyway, marked **loaded**, and the log says what was busy and which processes used the most CPU. machine-slot never pauses or closes anything on its own.

**`solo --ask`** lets it ask instead. If apps are busy when the solo's turn comes, a dialog lists them and offers to quit them for the run and reopen them afterwards. Apps quit the normal way, so they save and ask about unsaved work themselves; nothing is force-quit. The terminal or agent the solo runs under, Finder and the Dock are never offered; add more with `ask_keep` in the config. With no answer in 2 minutes it runs without closing anything. If machine-slot itself is killed, `machine-slot reopen` reopens what it closed.

**Progress.** A command may write its progress to the file named in `MACHINE_SLOT_PROGRESS`: «37», «37 %», «0.37» or «120/300». Without that, progress is estimated from the usual time and shown with a `~`.

**Time limits.** A solo that hangs would hold the whole machine, so a solo stops at 3 × its usual time (10 minutes at least) unless it says otherwise: `--expect 25m` says how long this run takes (the limit becomes 3 × that), `--timeout 20m` sets the limit itself, `--timeout 0` none. Shared jobs only stop with `--timeout`. A limit is there for hung jobs, not long ones:
- before the cut (5 minutes before at most), the job's output, `status` and the menu bar say so, and the menu bar notifies;
- `machine-slot extend LABEL [--by T]` (or **30 More Minutes** in its «…» menu) gives a running job more, 30 minutes by default;
- at a default limit, a job whose tree still works (CPU or disk in the last minute) gets extra time, half its limit (5 minutes at least), twice. A tree that has done nothing for a minute is stopped. A `--timeout` you chose gives no extra time.

 A stopped job exits 124, as timeout(1) does, and the record says it timed out. While a job runs, `status` and the menu bar also say when its process tree has used no CPU and no disk for 5 minutes (`stalled?`). It's a question, not a verdict: a UI test drives an app outside its own tree.

**First runs.** A solo with no history has no usual time to stop at, so it stops at 30 minutes unless it says otherwise. `--expect 15m` says how long it should take: it stands in for the usual time (progress, start times for the others, the time limit). It also wins over history, for a bench that changed under the same label.

**Lending the machine.** Everything waits while a solo runs. A long benchmark made of independent parts can call `machine-slot yield` between two of them (`--for 5m`, 3 minutes by default): waiting shared work whose usual time fits runs meanwhile, then the solo waits for it to end and for the machine to calm down, and its command goes on. The time lent isn't counted against the solo's run.

**Forcing.** `machine-slot go LABEL` (or **Start Now** in a job's «…» menu in the menu bar, which also has **Stop**) starts a waiting job now. A shared one starts beside whatever runs, and a solo measuring then is on record as not clean; a solo waiting for quiet stops waiting and is marked loaded. A solo can't jump in while another one runs: two at once would spoil both.

**Who.** `status` and the menu bar say where each job was started from, «folder · branch», or whatever `MACHINE_SLOT_OWNER` says (a session name, say).

**Later.** `--at 23:30` joins the queue only at that time, holding nothing until then: a long solo for tonight doesn't block anyone now.

**Squeezing in.** While a solo waits for running shared work to drain, a new shared job may still start, but only if its usual time ends before that work should. A job with no history waits.

**Nesting.** Inside a turn, `machine-slot` just runs the command: a shared inside a solo uses the solo's turn. A solo inside a shared is refused, because it would wait on itself.

**Labels.** Use the same label for the same kind of task: commit ids and numbers are dropped, so `nightly build 5978055` and `nightly build 62e67e7` count as one task. The same command in the same repository is also recognized as the same test whatever its label, from any worktree or commit.

Waiting can take a while. In scripts and agents, run it in the background.

## See what's going on

| command | |
|---|---|
| `machine-slot status` | what runs (% done, CPU / memory / disk right now), what waits and when it should start, and what a solo waiting for quiet is waiting on |
| `machine-slot estimate LABEL [solo\|shared]` | usual time, and how long it would wait now |
| `machine-slot times` | per task: runs, clean runs, usual time (clean / loaded), last, max, CPU, peak memory |
| `machine-slot repeats` | tests run more than once, with every label they ran under |
| `machine-slot log [N]` | the last N runs, one line each |
| `machine-slot state [--json]` | for scripts and the menu bar |
| `machine-slot go LABEL` | start a waiting job now (see Forcing) |
| `machine-slot extend LABEL [--by T]` | more time for a running job (see Time limits) |
| `machine-slot yield [--for T]` | inside a solo: lend the machine between two parts |
| `machine-slot cancel LABEL` | stop a job the way Ctrl-C would: its whole command, recorded, turn freed |
| `machine-slot wait LABEL` | wait for a job to end; exits as it did |
| `machine-slot paths` | where the queue, the record and the config live |
| `machine-slot mcp` | a read-only MCP server on stdio: `status`, `times`, `history`, `estimate` |

The **menu bar app** shows a lock with the time left while a solo runs, a stack with the number of shared jobs, and an hourglass with +N for waiting ones. Its menu lists running and waiting jobs, each with a progress bar (exact when the command reports it through `MACHINE_SLOT_PROGRESS`, estimated from its usual time otherwise) and, while it runs, the CPU, memory and disk its whole process tree is using right now. Waiting solos say when they should start. It reads the queue only when a job comes or goes, and every 2 s while something runs. Two switches in its menu, both off until you turn them on: **Open at Login**, and **Notify When Work Ends** (a solo, or anything that ran a minute or more; failures and runs on a busy machine are said as such).

## The record

Every run is one line in `runs.tsv` in the data folder (`machine-slot paths`):

| column | |
|---|---|
| `start`, `mode`, `key`, `label` | start time (Unix), solo/shared, task key, the label as given |
| `wait_s`, `run_s`, `exit` | time in the queue, wall time, exit code (128 + signal if stopped) |
| `cpu_user_s`, `cpu_sys_s` | CPU of the whole command tree |
| `max_rss_mb`, `peak_tree_mb` | peak RSS of its largest process; peak footprint of the whole tree, sampled every second |
| `load_start`, `concurrent_max` | 1-minute load at start; most other jobs seen running beside it |
| `clean` | 1 if its time means something on its own |
| `cmd_id`, `repo`, `head`, `command` | test identity, repository, commit, normalized command |
| `needs`, `settle_s`, `not_quiet`, `power` | solo only: what it asked for, seconds spent waiting for quiet, what was still busy, and thermal / low-power / battery notes |
| `closed_apps` | apps `--ask` quit for the run |
| `disk_mb`, `peak_cpu_pct` | disk read + written by the tree; its busiest second, in % of one core |

`times.txt` (the `times` table, rewritten after every run) and `log.txt` sit next to it.

**Clean runs.** CPU time and peak memory belong to the job and change little under load. Wall time doesn't. A solo is **clean** when the machine calmed down before it started. A shared job is clean when nothing else from the queue ran beside it and the load was low (under 0.3 × CPUs). Usual times are the median of the last 5 successful runs, using clean runs when there are any. Without them, loaded runs are scaled back to a free machine by the load they started at (CPU-bound work slows in proportion once the load passes the number of cores), and a shared job's estimate is scaled forward to the load right now. Only runs with no load on record make an estimate an upper bound, and it says so. Runs of a second or less did nothing and don't count.

**Test identity.** `cmd_id` is a hash of the repository (its git common dir, so every worktree is the same repository), the directory inside it, and the exact command, with the checkout's path written as `<repo>` and the home folder as `~`.

## Configuration

`~/.config/machine-slot/config` (or `$XDG_CONFIG_HOME/machine-slot/config`), one `key = value` per line:

| key | |
|---|---|
| `data_dir` | where the record lives (default `~/.local/share/machine-slot`) |
| `hook_heavy` | for the Claude Code hook: more heavy commands, comma-separated (`./build.sh, just test`) |
| `hook_timed` | more commands that measure, so they must be `solo` |
| `hook_allow` | tools that take their own turns |
| `ask_keep` | apps `--ask` never offers to quit (names or bundle ids) |

The `MACHINE_SLOT_STATE` and `MACHINE_SLOT_DIR` environment variables override the state and data folders. `MACHINE_SLOT_SETTLE=0` skips the wait for quiet (for CI).

## With AI coding agents

`integrations/claude-code/install.sh` sets up Claude Code for the current user:

- **A skill** that teaches any session to use the queue. Telling a session "use machine-slot" is enough.
- **A PreToolUse hook** (`machine-slot hook`, a few milliseconds per command) that refuses bare builds, tests and benchmarks, with the line to use instead: cargo, `xcodebuild`, `swift build/test`, make, ninja, `cmake --build`, meson, go, zig, bazel, gradle, maven, dotnet, npm/pnpm/yarn test and build, pytest, tox, nox and `hyperfine`, plus whatever `hook_heavy` adds. Benchmarks must be `solo`. Quoted text (commit messages, echo) and here-document bodies don't count.
- **The MCP server**, so a session can check the queue and estimates before it runs anything.

`--uninstall` removes all three. Other agents can call the CLI or the MCP server the same way.

## How it works

- **Jobs:** every job is a file under the state folder. Its owner keeps it locked with `flock` while it lives, so a job that crashes or is killed frees its place by itself, with no stale lock to clean up.
- **Decisions:** every decision is taken under one lock file. The policy is a single pure function, `sched::decide`, with unit tests.
- **Running:** the command runs in its own process group. Ctrl-C and TERM go to the whole group, and KILL follows after 10 s.
- **Measuring:** the run's CPU comes from `getrusage(RUSAGE_CHILDREN)`. Every second, each process of the tree is read for CPU (with its ended children's, so a build's short-lived compilers count), memory and disk; a process that starts and ends between two samples adds its CPU but not its disk. On macOS that's `proc_pid_rusage`, and the quiet checks read `host_statistics`, IOKit and CoreGraphics in place, starting no processes. On Linux it's `/proc` and PSI; there is no general way to read GPU use or keyboard idle time there, so `gpu` and the input part of `screen` aren't checked, and `--ask` finds no apps to ask about.

## Development

```bash
machine-slot solo "machine-slot: tests" -- cargo test --release   # its tests time things: run them alone
```

On a Mac, `scripts/linux-vm-test.sh` runs clippy and the tests on Linux in a lima VM of its own.

`tests/queue.rs` runs the real binary several at once in a private state; `tests/hook.rs` feeds the hook what Claude Code sends.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Contributions are accepted under the same terms.
