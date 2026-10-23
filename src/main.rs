//! CLI entry point.
//!
//! v0 shape (hand-rolled parsing on purpose: the surface is small and we
//! want zero dependencies beyond serde):
//!
//!   passerelle merge-prs 42 43 44 \
//!     --repo https://github.com/org/repo \
//!     --base main \
//!     --export-mode merge-topology
//!
//!   passerelle operator queue   # list pending conflicts
//!   passerelle operator submit --change <hash> --note "chose PR 42's approach"

use passerelle_git_pijul::{export, ingest, ledger, operator, translate, triage, ExportMode};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("merge-prs") => merge_prs(&args[1..]),
        Some("operator") => operator_cmd(&args[1..]),
        _ => {
            eprintln!("usage: passerelle <merge-prs|operator> [...]");
            std::process::exit(2);
        }
    }
}

fn merge_prs(args: &[String]) {
    let mut prs = Vec::new();
    let mut base = String::from("main");
    let mut export_mode = ExportMode::MergeTopology;
    let mut workdir = PathBuf::from(".gateway-work");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--base" => {
                base = args.get(i + 1).expect("--base needs a value").clone();
                i += 2;
            }
            "--export-mode" => {
                export_mode = match args.get(i + 1).map(String::as_str) {
                    Some("linear") => ExportMode::Linear,
                    Some("merge-topology") => ExportMode::MergeTopology,
                    other => panic!("unknown export mode: {other:?}"),
                };
                i += 2;
            }
            "--workdir" => {
                workdir = PathBuf::from(args.get(i + 1).expect("--workdir needs a value"));
                i += 2;
            }
            n => {
                prs.push(n.parse::<u64>().expect("PR number expected"));
                i += 1;
            }
        }
    }

    // Full pipeline wiring is the p0 milestone; the stages are called in
    // order so each TODO lands in a ready-made call site.
    println!("plan:\n  {}", export::plan(export_mode, &prs).join("\n  "));
    println!(
        "pipeline: ingest -> translate -> apply on integration channel -> triage -> operator -> export ({export_mode:?})"
    );

    let mut l = ledger::Ledger::load(&ledger::ledger_path(&workdir))
        .expect("ledger must load or be absent");
    // TODO(p0): for each PR: ingest::fetch_prs, translate::translate_pr.
    // TODO(p1): triage::triage_integration_channel, auto-resolve rules.
    // TODO(p2): operator::OperatorQueue for the rest.
    // TODO(p0): translate::export_integration, then push.
    let _ = (&mut l, &prs, base);
}

fn operator_cmd(args: &[String]) {
    let mut queue = operator::OperatorQueue::default();
    match args.first().map(String::as_str) {
        Some("queue") => {
            for p in &queue.pending {
                println!("conflict: {}", p.path);
            }
            println!("(operator console: p2 milestone)");
        }
        Some("submit") => {
            let mut hash = String::new();
            let mut note = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--change" => {
                        hash = args.get(i + 1).expect("--change needs a value").clone();
                        i += 2;
                    }
                    "--note" => {
                        note = Some(args.get(i + 1).expect("--note needs a value").clone());
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            queue.submit(operator::ResolutionChange {
                change_hash: hash,
                note,
            });
            println!("resolution queued (apply happens in the pipeline run)");
        }
        _ => {
            eprintln!("usage: passerelle operator <queue|submit>");
            std::process::exit(2);
        }
    }
    let _ = (&mut queue, &ingest::fetch_prs, &translate::translate_pr, &triage::triage_integration_channel);
}
