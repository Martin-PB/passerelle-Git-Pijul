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

fn git_rev_parse(repo: &std::path::Path, reference: &str) -> std::io::Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", &format!("{reference}^{{commit}}")])
        .current_dir(repo)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn merge_prs(args: &[String]) {
    let mut prs = Vec::new();
    let mut base = String::from("main");
    let mut export_mode = ExportMode::MergeTopology;
    let mut workdir = PathBuf::from(".gateway-work");
    let mut mirror = PathBuf::from(".gateway-mirror");

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
            "--mirror" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--mirror needs a value");
                    std::process::exit(2);
                };
                mirror = PathBuf::from(value);
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

    if prs.is_empty() {
        eprintln!("merge-prs requires at least one PR number");
        std::process::exit(2);
    }
    println!("plan:\n  {}", export::plan(export_mode, &prs).join("\n  "));

    let base_ref = if base.starts_with("refs/") {
        base.clone()
    } else {
        format!("refs/remotes/base/{base}")
    };
    let base_sha = git_rev_parse(&mirror, &base_ref).unwrap_or_else(|error| {
        eprintln!("cannot resolve base ref {base_ref}: {error}");
        std::process::exit(1);
    });
    let base_info = passerelle_git_pijul::Base {
        branch: base.clone(),
        sha: base_sha,
    };
    let mut l = ledger::Ledger::load(&ledger::ledger_path(&workdir))
        .unwrap_or_else(|error| {
            eprintln!("ledger load failed: {error}");
            std::process::exit(1);
        });
    let pijul_repo = workdir.join("pijul");
    let scratch = workdir.join("scratch");
    for number in &prs {
        let head_ref = format!("refs/remotes/pr/{number}");
        let head_sha = git_rev_parse(&mirror, &head_ref).unwrap_or_else(|error| {
            eprintln!("cannot resolve PR {number} ref {head_ref}: {error}");
            std::process::exit(1);
        });
        let pr = passerelle_git_pijul::PullRequest {
            number: *number,
            title: format!("PR {number}"),
            author: "unknown".into(),
            head_sha,
            head_ref,
        };
        if let Err(error) = translate::translate_pr_from_mirror(
            &mirror,
            &pijul_repo,
            &scratch,
            &base_info,
            &pr,
            &mut l,
        ) {
            eprintln!("translation of PR {number} failed: {error}");
            std::process::exit(1);
        }
        println!("imported PR {number} into Pijul channel pr-{number}");
    }

    println!("Pijul integration channel prepared.");
    println!("triage/operator resolution and Git export remain unimplemented; no Git branch was written.");
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
