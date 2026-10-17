//! Argument parsing, step timing and the summary table of `cargo xtask ci`, and the
//! `changed-examples` helper. Nothing here spawns a process, so it is unit-tested.

use std::collections::BTreeSet;
use std::time::Duration;

/// What `cargo xtask ci` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiMode {
    /// The full local gate: the one to run before merging.
    Full,
    /// `--fast`: fmt, clippy and the workspace tests with `ZYNQSIM_FAST=1`. Does NOT
    /// replace the full gate.
    Fast,
    /// `--det-only a,b`: only the determinism test, restricted to these examples.
    DetOnly(Vec<String>),
}

/// Parses the arguments that follow `ci`.
pub fn parse_ci_args(args: &[String]) -> Result<CiMode, String> {
    let mut fast = false;
    let mut det: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--fast" => fast = true,
            "--det-only" => {
                i += 1;
                let list = args
                    .get(i)
                    .ok_or("--det-only needs a comma-separated list of examples")?;
                det = Some(parse_names(list)?);
            }
            other => match other.strip_prefix("--det-only=") {
                Some(list) => det = Some(parse_names(list)?),
                None => return Err(format!("unknown option `{other}` for `ci`")),
            },
        }
        i += 1;
    }
    match (fast, det) {
        (true, Some(_)) => Err("--fast and --det-only cannot be combined".into()),
        (true, None) => Ok(CiMode::Fast),
        (false, Some(names)) => Ok(CiMode::DetOnly(names)),
        (false, None) => Ok(CiMode::Full),
    }
}

/// Splits a comma-separated list of example names.
pub fn parse_names(list: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = list
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect();
    if names.is_empty() {
        Err("--det-only needs at least one example name".into())
    } else {
        Ok(names)
    }
}

/// `3m12s`, `1h02m05s`, `42s`.
pub fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// One finished (or failed) step of the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    /// Label as printed (`fmt`, `test --workspace`, ...).
    pub name: String,
    /// Wall time from the start to the end of the step.
    pub elapsed: Duration,
    /// Whether the step succeeded.
    pub ok: bool,
}

/// The per-step table printed at the end of the gate, pass or fail.
pub fn summary_table(steps: &[StepRecord], total: Duration) -> String {
    let width = steps.iter().map(|s| s.name.len()).max().unwrap_or(4).max(5);
    let mut out = format!("{:<width$}  {:>10}  result\n", "step", "duration");
    for s in steps {
        out.push_str(&format!(
            "{:<width$}  {:>10}  {}\n",
            s.name,
            fmt_duration(s.elapsed),
            if s.ok { "ok" } else { "FAILED" }
        ));
    }
    out.push_str(&format!(
        "{:<width$}  {:>10}\n",
        "total",
        fmt_duration(total)
    ));
    out
}

/// Example names touched by a list of changed paths (as `git diff --name-only` prints
/// them, relative to the zynqsim directory): every `examples/<name>/...` path.
pub fn changed_examples(paths: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in paths.lines() {
        let path = line.trim().replace('\\', "/");
        if let Some(rest) = path.strip_prefix("examples/") {
            if let Some((name, _)) = rest.split_once('/') {
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_is_the_full_gate() {
        assert_eq!(parse_ci_args(&args(&[])), Ok(CiMode::Full));
    }

    #[test]
    fn fast_flag_selects_the_fast_gate() {
        assert_eq!(parse_ci_args(&args(&["--fast"])), Ok(CiMode::Fast));
    }

    #[test]
    fn det_only_takes_a_separate_or_an_equals_value() {
        let want = CiMode::DetOnly(vec!["demo".into(), "math-qemu".into()]);
        assert_eq!(
            parse_ci_args(&args(&["--det-only", "demo, math-qemu,"])),
            Ok(want.clone())
        );
        assert_eq!(
            parse_ci_args(&args(&["--det-only=demo,math-qemu"])),
            Ok(want)
        );
    }

    #[test]
    fn det_only_needs_names() {
        assert!(parse_ci_args(&args(&["--det-only"])).is_err());
        assert!(parse_ci_args(&args(&["--det-only="])).is_err());
        assert!(parse_ci_args(&args(&["--det-only", " , "])).is_err());
    }

    #[test]
    fn fast_and_det_only_are_exclusive_and_unknown_options_fail() {
        assert!(parse_ci_args(&args(&["--fast", "--det-only", "demo"])).is_err());
        assert!(parse_ci_args(&args(&["--slow"])).is_err());
        assert!(parse_ci_args(&args(&["demo"])).is_err());
    }

    #[test]
    fn durations_read_like_the_step_lines() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0s");
        assert_eq!(fmt_duration(Duration::from_millis(42_900)), "42s");
        assert_eq!(fmt_duration(Duration::from_secs(192)), "3m12s");
        assert_eq!(fmt_duration(Duration::from_secs(3725)), "1h02m05s");
    }

    #[test]
    fn summary_lists_every_step_and_the_total() {
        let steps = [
            StepRecord {
                name: "fmt".into(),
                elapsed: Duration::from_secs(2),
                ok: true,
            },
            StepRecord {
                name: "test --workspace".into(),
                elapsed: Duration::from_secs(192),
                ok: false,
            },
        ];
        let table = summary_table(&steps, Duration::from_secs(200));
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("step"));
        assert!(lines[1].starts_with("fmt") && lines[1].ends_with("ok"));
        assert!(lines[2].contains("3m12s") && lines[2].ends_with("FAILED"));
        assert!(lines[3].starts_with("total") && lines[3].contains("3m20s"));
    }

    #[test]
    fn changed_examples_keeps_the_example_directories_only() {
        let paths = "examples/demo/project.toml\n\
                     examples/demo/main.v\n\
                     examples\\math-qemu\\project.toml\n\
                     crates/cli/src/main.rs\n\
                     examples/README.md\n\
                     \n";
        assert_eq!(
            changed_examples(paths),
            vec!["demo".to_string(), "math-qemu".to_string()]
        );
        assert!(changed_examples("docs/TESTPLAN.md\n").is_empty());
    }
}
