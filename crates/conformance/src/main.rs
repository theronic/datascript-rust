//! The port's side of the conformance harness. `conformance run cases.edn out` runs a file of cases and prints, for
//! every step, what the oracle prints (conformance/oracle: ClojureScript DataScript itself, over the same file);
//! `conformance compare oracle.out rust.out` tells where the two differ.
//!
//! A case is one line of EDN: a vector of steps, each a map with an `:op`. A step may name its result (`:as`), which
//! later steps of the case use as `#r name`; `#f name` is one of the functions both sides implement alike.

// A value keeps its hash once it is computed, and that is all it ever changes of itself: neither its hash nor what it
// is equal to moves, so it is a sound key, whatever clippy makes of the cell.
#![allow(clippy::mutable_key_type)]
// The types and argument lists are the ones DataScript's own functions have.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod fns;
mod steps;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") if args.len() == 3 => {
            let input = match std::fs::read_to_string(&args[1]) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("conformance: cannot read {}: {e}", args[1]);
                    return ExitCode::from(2);
                }
            };
            let out = steps::run_file(&input);
            if let Err(e) = std::fs::write(&args[2], out) {
                eprintln!("conformance: cannot write {}: {e}", args[2]);
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Some("compare") if args.len() >= 3 => compare(&args[1], &args[2], args.get(3).map(String::as_str)),
        _ => {
            eprintln!(
                "usage: conformance run <cases.edn> <out> | conformance compare <oracle.out> <rust.out> [cases.edn]"
            );
            ExitCode::from(2)
        }
    }
}

/// Whether the port's line answers as the oracle's does. An error ClojureScript raised without data is JavaScript's
/// own, whose wording is the engine's: any error answers it.
fn same(oracle: &str, rust: &str) -> bool {
    oracle == rust
        // a step that one side of the comparison has no way to run
        || rust == "#skipped"
        || (oracle.starts_with("#error :native") && rust.starts_with("#error"))
        || (oracle.contains(AUTO_TEMPID) && unnumbered(oracle) == unnumbered(rust))
}

const AUTO_TEMPID: &str = "#datascript/AutoTempid [";

/// A line with the numbers of its automatic tempids left out. They come from a counter that every transaction of
/// the process draws from, and ClojureScript, numbering lazily, draws less often: the numbers tell nothing.
fn unnumbered(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(AUTO_TEMPID) {
        let after = at + AUTO_TEMPID.len();
        out.push_str(&rest[..after]);
        rest = rest[after..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

fn compare(oracle: &str, rust: &str, cases: Option<&str>) -> ExitCode {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {p}: {e}"));
    let (oracle, rust) = (read(oracle), read(rust));
    let case_lines: Vec<String> = cases
        .map(|p| read(p).lines().filter(|l| !l.trim().is_empty() && !l.starts_with(';')).map(String::from).collect())
        .unwrap_or_default();
    let split = |s: &str| -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = Vec::new();
        for line in s.lines() {
            if line.starts_with("== ") {
                out.push(Vec::new());
            } else if let Some(last) = out.last_mut() {
                last.push(line.to_string());
            }
        }
        out
    };
    let (o, r) = (split(&oracle), split(&rust));
    let mut steps = 0usize;
    let mut bad_steps = 0usize;
    let mut bad_cases = 0usize;
    let shown_limit = std::env::var("CONFORMANCE_SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(12usize);
    for i in 0..o.len().max(r.len()) {
        let (a, b) = (o.get(i), r.get(i));
        let (Some(a), Some(b)) = (a, b) else {
            bad_cases += 1;
            println!("case {i}: missing on one side");
            continue;
        };
        let mut bad = false;
        for j in 0..a.len().max(b.len()) {
            steps += 1;
            let (x, y) =
                (a.get(j).map(String::as_str).unwrap_or("<none>"), b.get(j).map(String::as_str).unwrap_or("<none>"));
            if !same(x, y) {
                bad_steps += 1;
                if !bad && bad_cases < shown_limit {
                    println!("case {i}, step {j}:");
                    if let Some(c) = case_lines.get(i) {
                        println!("  case:   {}", truncate(c, 1500));
                    }
                    println!("  oracle: {}", truncate(x, 1500));
                    println!("  rust:   {}", truncate(y, 1500));
                }
                bad = true;
            }
        }
        if bad {
            bad_cases += 1;
        }
    }
    println!("{} cases, {} steps: {} steps differ in {} cases", o.len().max(r.len()), steps, bad_steps, bad_cases);
    if bad_steps == 0 && bad_cases == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}
