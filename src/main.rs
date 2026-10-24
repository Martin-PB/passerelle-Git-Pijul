//! CLI entry point.
//!
//! The generic `ingest` command is the hosting-agnostic foundation. The
//! higher-level merge pipeline remains incomplete until the Pijul translation
//! and Git export stages are implemented.
//!
//!   passerelle ingest --remote ../origin --workdir .gateway-mirror \
//!     --ref refs/heads/main=refs/remotes/base/main \
//!     --ref refs/heads/feature-a=refs/remotes/incoming/feature-a
//!
//!   passerelle merge-prs 42 43 --base main --export-mode merge-topology
//!   passerelle operator queue
//!   passerelle operator submit --change <hash> --note "chose branch A"

use passerelle_git_pijul::{export, ingest, ledger, operator, translate, triage, ExportMode};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ingest") => ingest_cmd(&args[1..]),
        Some("merge-prs") => merge_prs(&args[1..]),
        Some("operator") => operator_cmd(&args[1..]),
        _ => {
            eprintln!("usage: passerelle <ingest|merge-prs|operator> [...]");
            std::process::exit(2);
        }
    }
}

/// Fetch explicit generic refs into a bare mirror, without assuming GitHub,
/// PR numbers, or a particular hosting provider.
fn ingest_cmd(args: &[String]) {
    let mut remote = None;
    let mut workdir = PathBuf::from(".gateway-mirror");
    let mut refs: Vec<(String, String)> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--remote" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--remote needs a value");
                    std::process::exit(2);
                };
                remote = Some(value.clone());
                i += 2;
            }
            "--workdir" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--workdir needs a value");
                    std::process::exit(2);
                };
                workdir = PathBuf::from(value);
                i += 2;
            }
            "--ref" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--ref needs source=destination");
                    std::process::exit(2);
                };
                let Some((source, destination)) = value.split_once('=') else {
                    eprintln!("invalid --ref {value:?}; expected source=destination");
                    std::process::exit(2);
                };
                refs.push((source.to_owned(), destination.to_owned()));
                i += 2;
            }
            other => {
                eprintln!("unknown ingest argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let Some(remote) = remote else {
        eprintln!("ingest requires --remote");
        std::process::exit(2);
    };
    if refs.is_empty() {
        eprintln!("ingest requires at least one --ref source=destination");
        std::process::exit(2);
    }

    match ingest::fetch_refs(&remote, &refs, &workdir) {
        Ok(()) => {
            println!("fetched {} ref(s) into bare mirror {}", refs.len(), workdir.display());
        }
        Err(error) => {
            eprintln!("ingest failed: {error}");
            std::process::exit(1);
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
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--base needs a value");
                    std::process::exit(2);
                };
                base = value.clone();
                i += 2;
            }
            "--export-mode" => {
                export_mode = match args.get(i + 1).map(String::as_str) {
                    Some("linear") => ExportMode::Linear,
                    Some("merge-topology") => ExportMode::MergeTopology,
                    other => {
                        eprintln!("unknown export mode: {other:?}");
                        std::process::exit(2);
                    }
                };
                i += 2;
            }
            "--workdir" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--workdir needs a value");
                    std::process::exit(2);
                };
                workdir = PathBuf::from(value);
                i += 2;
            }
            n => match n.parse::<u64>() {
                Ok(number) => {
                    prs.push(number);
                    i += 1;
                }
                Err(_) => {
                    eprintln!("unexpected merge-prs argument: {n}");
                    std::process::exit(2);
                }
            },
        }
    }

    println!("plan:\n  {}", export::plan(export_mode, &prs).join("\n  "));
    println!(
        "pipeline not yet executable end-to-end: ingest -> translate -> apply -> triage -> operator -> export ({export_mode:?})"
    );

    let l = ledger::Ledger::load(&ledger::ledger_path(&workdir))
        .unwrap_or_else(|error| {
            eprintln!("ledger load failed: {error}");
            std::process::exit(1);
        });
    // TODO(P0): wire generic refs to translate::translate_pr and export.
    // TODO(P1): triage and deterministic auto-resolution.
    // TODO(P2): persistent operator queue.
    let _ = (&l, &prs, base);
}

fn operator_cmd(args: &[String]) {
    let mut queue = operator::OperatorQueue::default();
    match args.first().map(String::as_str) {
        Some("queue") => {
            for pending in &queue.pending {
                println!("conflict: {}", pending.path);
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
                        let Some(value) = args.get(i + 1) else {
                            eprintln!("--change needs a value");
                            std::process::exit(2);
                        };
                        hash = value.clone();
                        i += 2;
                    }
                    "--note" => {
                        let Some(value) = args.get(i + 1) else {
                            eprintln!("--note needs a value");
                            std::process::exit(2);
                        };
                        note = Some(value.clone());
                        i += 2;
                    }
                    other => {
                        eprintln!("unknown operator submit argument: {other}");
                        std::process::exit(2);
                    }
                }
            }
            if hash.trim().is_empty() {
                eprintln!("operator submit requires --change <hash>");
                std::process::exit(2);
            }
            queue.submit(operator::ResolutionChange {
                change_hash: hash,
                note,
            });
            println!("resolution queued in memory only (persistent queue is not implemented)");
        }
        _ => {
            eprintln!("usage: passerelle operator <queue|submit> [...]");
            std::process::exit(2);
        }
    }
    let _ = (&mut queue, &translate::translate_pr, &triage::triage_integration_channel);
}
