//! The record of every run (runs.tsv), task keys, and usual times.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub const HEADER: &str = "start\tmode\tkey\twait_s\trun_s\texit\tlabel\tcpu_user_s\tcpu_sys_s\tmax_rss_mb\tpeak_tree_mb\tload_start\tconcurrent_max\tclean\tcmd_id\trepo\thead\tcommand\tneeds\tsettle_s\tnot_quiet\tpower\tclosed_apps\tdisk_mb\tpeak_cpu_pct";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub start: u64,
    pub mode: String,
    pub key: String,
    pub wait_s: u64,
    pub run_s: u64,
    pub exit: i32,
    pub label: String,
    pub cpu_user_s: Option<f64>,
    pub cpu_sys_s: Option<f64>,
    pub max_rss_mb: Option<u64>,
    pub peak_tree_mb: Option<u64>,
    pub load_start: Option<f64>,
    pub concurrent_max: Option<u32>,
    /// Ran with no other slot work and a quiet machine: its time means
    /// something on its own. Unknown (old rows) counts as not clean.
    pub clean: bool,
    /// The same test whatever its label: a hash of the repo, the directory in
    /// it and the exact command (see `fingerprint`). Empty for old rows.
    pub cmd_id: String,
    pub repo: String,
    /// The commit it ran on, for context; not part of the identity.
    pub head: String,
    pub command: String,
    /// Solo only: what it asked to be quiet (cpu,disk,ram,gpu,screen), how
    /// long it waited for that, what still wasn't quiet when it started
    /// (empty = calm), and thermal / low power / battery notes.
    pub needs: String,
    pub settle_s: Option<u64>,
    pub not_quiet: String,
    pub power: String,
    /// Apps quit for the run with the user's OK (`--ask`), reopened after.
    pub closed_apps: String,
    /// Disk read + written by the whole tree, MB.
    pub disk_mb: Option<u64>,
    /// The busiest second, % of one core.
    pub peak_cpu_pct: Option<u64>,
}

impl Run {
    pub fn to_line(&self) -> String {
        let o = |v: Option<String>| v.unwrap_or_default();
        [
            self.start.to_string(),
            self.mode.clone(),
            self.key.clone(),
            self.wait_s.to_string(),
            self.run_s.to_string(),
            self.exit.to_string(),
            self.label.clone(),
            o(self.cpu_user_s.map(|v| format!("{v:.2}"))),
            o(self.cpu_sys_s.map(|v| format!("{v:.2}"))),
            o(self.max_rss_mb.map(|v| v.to_string())),
            o(self.peak_tree_mb.map(|v| v.to_string())),
            o(self.load_start.map(|v| format!("{v:.2}"))),
            o(self.concurrent_max.map(|v| v.to_string())),
            (self.clean as u8).to_string(),
            self.cmd_id.clone(),
            self.repo.clone(),
            self.head.clone(),
            self.command.replace(['\t', '\n'], " "),
            self.needs.clone(),
            o(self.settle_s.map(|v| v.to_string())),
            self.not_quiet.replace(['\t', '\n'], " "),
            self.power.clone(),
            self.closed_apps.clone(),
            o(self.disk_mb.map(|v| v.to_string())),
            o(self.peak_cpu_pct.map(|v| v.to_string())),
        ]
        .join("\t")
    }

    /// A row of runs.tsv; rows from older versions have fewer fields.
    pub fn parse(line: &str) -> Option<Run> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 || f[0] == "start" {
            return None;
        }
        let get = |i: usize| f.get(i).copied().filter(|s| !s.is_empty());
        Some(Run {
            start: f[0].parse().ok()?,
            mode: f[1].to_string(),
            key: f[2].to_string(),
            wait_s: f[3].parse().ok()?,
            run_s: f[4].parse().ok()?,
            exit: f[5].parse().ok()?,
            label: f[6].to_string(),
            cpu_user_s: get(7).and_then(|s| s.parse().ok()),
            cpu_sys_s: get(8).and_then(|s| s.parse().ok()),
            max_rss_mb: get(9).and_then(|s| s.parse().ok()),
            peak_tree_mb: get(10).and_then(|s| s.parse().ok()),
            load_start: get(11).and_then(|s| s.parse().ok()),
            concurrent_max: get(12).and_then(|s| s.parse().ok()),
            clean: get(13) == Some("1"),
            cmd_id: get(14).unwrap_or_default().to_string(),
            repo: get(15).unwrap_or_default().to_string(),
            head: get(16).unwrap_or_default().to_string(),
            command: get(17).unwrap_or_default().to_string(),
            needs: get(18).unwrap_or_default().to_string(),
            settle_s: get(19).and_then(|s| s.parse().ok()),
            not_quiet: get(20).unwrap_or_default().to_string(),
            power: get(21).unwrap_or_default().to_string(),
            closed_apps: get(22).unwrap_or_default().to_string(),
            disk_mb: get(23).and_then(|s| s.parse().ok()),
            peak_cpu_pct: get(24).and_then(|s| s.parse().ok()),
        })
    }
}

/// «nightly build 5978055» and «nightly build 62e67e7» are the same kind of task:
/// commit ids and numbers go.
pub fn key_of(label: &str) -> String {
    label
        .split_whitespace()
        .filter(|w| {
            let t = w.trim_matches(|c: char| !c.is_alphanumeric());
            let hex = t.len() >= 7
                && t.len() <= 40
                && t.chars().all(|c| c.is_ascii_hexdigit())
                && t.chars().any(|c| c.is_ascii_digit());
            let num = !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
            !(hex || num)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn read(path: &Path) -> Vec<Run> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().filter_map(Run::parse).collect())
        .unwrap_or_default()
}

/// One write with O_APPEND: whole lines, even with several writers.
pub fn append(path: &Path, run: &Run) -> std::io::Result<()> {
    let fresh = !path.exists()
        || std::fs::metadata(path)
            .map(|m| m.len() == 0)
            .unwrap_or(true);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut text = String::new();
    if fresh {
        text.push_str(HEADER);
        text.push('\n');
    }
    text.push_str(&run.to_line());
    text.push('\n');
    f.write_all(text.as_bytes())
}

fn median(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usual {
    pub secs: u64,
    /// From runs on a loaded machine only: an upper bound, not the real time.
    pub loaded: bool,
}

/// Median of the last 5 good runs of this kind, preferring clean ones.
/// Runs this short or shorter don't count toward a task's usual time.
const NO_WORK_S: u64 = 1;

pub fn usual(runs: &[Run], mode: &str, key: &str) -> Option<Usual> {
    usual_where(runs, |r| r.mode == mode && r.key == key)
}

/// The same, for the same test (by `cmd_id`) first, else by label.
pub fn usual_for(runs: &[Run], mode: &str, cmd_id: &str, key: &str) -> Option<Usual> {
    (!cmd_id.is_empty())
        .then(|| usual_where(runs, |r| r.mode == mode && r.cmd_id == cmd_id))
        .flatten()
        .or_else(|| usual(runs, mode, key))
}

fn usual_where(runs: &[Run], pick: impl Fn(&Run) -> bool) -> Option<Usual> {
    let mut good: Vec<&Run> = runs.iter().filter(|r| pick(r) && r.exit == 0).collect();
    // A run of a second or less did no work (everything cached, no test
    // matched): it says nothing about how long the task takes, unless that's
    // all the task ever does.
    if good.iter().any(|r| r.run_s > NO_WORK_S) {
        good.retain(|r| r.run_s > NO_WORK_S);
    }
    let last5 = |v: Vec<&Run>| median(v.iter().rev().take(5).map(|r| r.run_s).collect());
    if let Some(secs) = last5(good.iter().copied().filter(|r| r.clean).collect()) {
        return Some(Usual {
            secs,
            loaded: false,
        });
    }
    last5(good).map(|secs| Usual { secs, loaded: true })
}

/// FNV-1a, 64 bits: stable across runs and builds (std's hasher isn't).
pub fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

/// The identity of a test: which repository (every worktree of a repo is
/// the same one), where in it, and the exact command, with the checkout's
/// own path and the home folder written generically so worktrees and
/// scratch paths under them don't make it a different test.
pub fn fingerprint(
    repo: &str,
    toplevel: &str,
    rel_dir: &str,
    home: &str,
    argv: &[String],
) -> (String, String) {
    let norm = |a: &str| {
        let a = if !toplevel.is_empty() {
            a.replace(toplevel, "<repo>")
        } else {
            a.to_string()
        };
        if !home.is_empty() {
            a.replace(home, "~")
        } else {
            a
        }
    };
    let command: Vec<String> = argv.iter().map(|a| norm(a)).collect();
    let mut parts = vec![repo, rel_dir];
    parts.extend(command.iter().map(String::as_str));
    (fnv(&parts), command.join(" "))
}

/// Tests run more than once: one line per identity, with how many runs,
/// their usual time and every label they ran under.
pub fn repeats(runs: &[Run]) -> String {
    let mut ids: Vec<&str> = runs
        .iter()
        .filter(|r| !r.cmd_id.is_empty())
        .map(|r| r.cmd_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut rows: Vec<(usize, String)> = Vec::new();
    for id in ids {
        let rs: Vec<&Run> = runs.iter().filter(|r| r.cmd_id == id).collect();
        if rs.len() < 2 {
            continue;
        }
        let mut labels: Vec<&str> = rs.iter().map(|r| r.label.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        let mut heads: Vec<&str> = rs
            .iter()
            .map(|r| r.head.as_str())
            .filter(|h| !h.is_empty())
            .collect();
        heads.sort_unstable();
        heads.dedup();
        let last = rs.last().unwrap();
        let u = usual_where(runs, |r| r.cmd_id == id && r.mode == last.mode);
        rows.push((
            rs.len(),
            format!(
                "{id}  {} runs, usual {}, {} commits, {} fails\n  repo:    {}\n  command: {}\n  labels:  {}\n",
                rs.len(),
                u.map(|u| format!("~{}{}", clock(u.secs), if u.loaded { " (loaded)" } else { "" })).unwrap_or_else(|| "-".into()),
                heads.len(),
                rs.iter().filter(|r| r.exit != 0).count(),
                last.repo,
                last.command,
                labels.join(" · ")
            ),
        ));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    if rows.is_empty() {
        return "no test has run twice yet (old rows have no identity)\n".into();
    }
    rows.into_iter().map(|r| r.1).collect()
}

pub fn clock(s: u64) -> String {
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Per kind of task: runs, clean runs, usual (clean / loaded), last, max,
/// mean CPU, peak memory.
pub fn times_table(runs: &[Run]) -> String {
    let mut kinds: Vec<(String, String)> = runs
        .iter()
        .map(|r| (r.mode.clone(), r.key.clone()))
        .collect();
    kinds.sort();
    kinds.dedup();
    let mut out = format!(
        "{:<6} {:>4} {:>5} {:>8} {:>8} {:>7} {:>7} {:>7} {:>7}  {}\n",
        "mode", "runs", "clean", "usual", "loaded", "last", "max", "cpu", "peakMB", "task"
    );
    for (m, k) in kinds {
        let rs: Vec<&Run> = runs.iter().filter(|r| r.mode == m && r.key == k).collect();
        let good = |clean: bool| {
            median(
                rs.iter()
                    .filter(|r| r.exit == 0 && r.clean == clean)
                    .rev()
                    .take(5)
                    .map(|r| r.run_s)
                    .collect(),
            )
        };
        let cpus: Vec<f64> = rs
            .iter()
            .filter_map(|r| Some(r.cpu_user_s? + r.cpu_sys_s?))
            .collect();
        let cpu = (!cpus.is_empty())
            .then(|| clock((cpus.iter().sum::<f64>() / cpus.len() as f64).round() as u64));
        let mem = rs
            .iter()
            .filter_map(|r| r.peak_tree_mb.or(r.max_rss_mb))
            .max();
        let dash = |o: Option<String>| o.unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "{:<6} {:>4} {:>5} {:>8} {:>8} {:>7} {:>7} {:>7} {:>7}  {}\n",
            m,
            rs.len(),
            rs.iter().filter(|r| r.clean).count(),
            dash(good(true).map(clock)),
            dash(good(false).map(clock)),
            clock(rs.last().map(|r| r.run_s).unwrap_or(0)),
            clock(rs.iter().map(|r| r.run_s).max().unwrap_or(0)),
            dash(cpu),
            dash(mem.map(|v| v.to_string())),
            k
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(key: &str, secs: u64, exit: i32, clean: bool) -> Run {
        Run {
            mode: "shared".into(),
            key: key.into(),
            run_s: secs,
            exit,
            clean,
            label: key.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_same_test_is_one_identity_whatever_its_label_or_worktree() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let repo = "/home/ana/photo-app/.git";
        let (a, cmd) = fingerprint(
            repo,
            "/home/ana/photo-app",
            "store",
            "/home/ana",
            &argv(&["cargo", "test", "-p", "store"]),
        );
        let (b, _) = fingerprint(
            repo,
            "/home/ana/photo-app-worktrees/x",
            "store",
            "/home/ana",
            &argv(&["cargo", "test", "-p", "store"]),
        );
        assert_eq!(a, b, "another worktree of the same repo");
        assert_eq!(cmd, "cargo test -p store");
        let (c, _) = fingerprint(
            repo,
            "/home/ana/photo-app",
            "store",
            "/home/ana",
            &argv(&["cargo", "test", "-p", "ffi"]),
        );
        assert_ne!(a, c, "another crate is another test");
        let (d, _) = fingerprint(
            repo,
            "/home/ana/photo-app",
            "",
            "/home/ana",
            &argv(&["cargo", "test", "-p", "store"]),
        );
        assert_ne!(a, d, "run from another directory of the repo");
        let (e, ecmd) = fingerprint(
            repo,
            "/home/ana/photo-app-worktrees/x",
            "",
            "/home/ana",
            &argv(&["run", "--out", "/home/ana/photo-app-worktrees/x/out"]),
        );
        assert_eq!(ecmd, "run --out <repo>/out");
        let (f, _) = fingerprint(
            repo,
            "/home/ana/photo-app",
            "",
            "/home/ana",
            &argv(&["run", "--out", "/home/ana/photo-app/out"]),
        );
        assert_eq!(e, f);
        assert_eq!(fnv(&["a", "b"]), fnv(&["a", "b"]));
        assert_ne!(fnv(&["ab"]), fnv(&["a", "b"]));
    }

    #[test]
    fn estimates_follow_the_test_not_the_label() {
        let mut rs = vec![
            run("label one", 50, 0, true),
            run("label two", 52, 0, true),
            run("label two", 9, 0, true),
        ];
        rs[0].cmd_id = "id".into();
        rs[1].cmd_id = "id".into();
        assert_eq!(
            usual_for(&rs, "shared", "id", "label two").unwrap().secs,
            52
        );
        assert_eq!(usual_for(&rs, "shared", "", "label two").unwrap().secs, 52);
        assert_eq!(
            usual_for(&rs, "shared", "new", "label two").unwrap().secs,
            52,
            "no runs of it: by label"
        );
        let rep = repeats(&rs);
        assert!(
            rep.contains("2 runs") && rep.contains("label one · label two"),
            "{rep}"
        );
    }

    #[test]
    fn keys_drop_commits_and_numbers() {
        assert_eq!(key_of("nightly build 5978055"), "nightly build");
        assert_eq!(key_of("ui-test app 62e67e7"), "ui-test app");
        assert_eq!(
            key_of("engine: baseline run 3 of 5"),
            "engine: baseline run of"
        );
        assert_eq!(
            key_of("store: cargo test -p store"),
            "store: cargo test -p store"
        );
        // A word that only looks hex is kept.
        assert_eq!(
            key_of("slicer: deadbeefcafe facade"),
            "slicer: deadbeefcafe facade"
        );
        assert_eq!(key_of("x2 benchy.3mf"), "x2 benchy.3mf");
    }

    #[test]
    fn usual_prefers_clean_runs_and_ignores_failures() {
        let rs = vec![
            run("a", 100, 0, false),
            run("a", 90, 0, false),
            run("a", 10, 0, true),
            run("a", 12, 0, true),
            run("a", 1, 1, true),
        ];
        assert_eq!(
            usual(&rs, "shared", "a"),
            Some(Usual {
                secs: 12,
                loaded: false
            })
        );
        let loaded = vec![
            run("b", 100, 0, false),
            run("b", 80, 0, false),
            run("b", 90, 0, false),
        ];
        assert_eq!(
            usual(&loaded, "shared", "b"),
            Some(Usual {
                secs: 90,
                loaded: true
            })
        );
        assert_eq!(usual(&rs, "solo", "a"), None);
    }

    #[test]
    fn usual_is_the_median_of_the_last_five() {
        let rs: Vec<Run> = [500, 11, 12, 13, 14, 15]
            .iter()
            .map(|&s| run("a", s, 0, true))
            .collect();
        assert_eq!(usual(&rs, "shared", "a").unwrap().secs, 13);
    }

    #[test]
    fn runs_that_did_no_work_dont_make_the_usual_time() {
        let rs: Vec<Run> = [70, 0, 0, 1, 68, 0]
            .iter()
            .map(|&s| run("a", s, 0, true))
            .collect();
        assert_eq!(usual(&rs, "shared", "a").unwrap().secs, 70);
        // A task that is always instant is instant.
        let quick: Vec<Run> = [0, 1, 0].iter().map(|&s| run("q", s, 0, true)).collect();
        assert_eq!(usual(&quick, "shared", "q").unwrap().secs, 0);
    }

    #[test]
    fn rows_round_trip_and_old_rows_parse() {
        let r = Run {
            start: 1793224847,
            mode: "solo".into(),
            key: "k".into(),
            wait_s: 3,
            run_s: 40,
            exit: 0,
            label: "k 1".into(),
            cpu_user_s: Some(12.5),
            cpu_sys_s: Some(1.25),
            max_rss_mb: Some(300),
            peak_tree_mb: Some(512),
            load_start: Some(2.5),
            concurrent_max: Some(0),
            clean: true,
            ..Default::default()
        };
        assert_eq!(Run::parse(&r.to_line()), Some(r));
        let old =
            Run::parse("1793224847\tshared\tstore: ffi tests\t0\t18\t0\tstore: ffi tests").unwrap();
        assert_eq!(old.run_s, 18);
        assert!(!old.clean && old.cpu_user_s.is_none());
        assert!(Run::parse(HEADER).is_none());
    }
}
