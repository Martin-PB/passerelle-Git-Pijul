//! Operator boundary: the gateway's ONLY interface towards humans (and
//! whatever assists them).
//!
//! The contract is minimal and closed:
//!   the gateway EXPOSES   -> ConflictReport (a Pijul conflict state + the
//!                            context of the conflicting PRs)
//!   the gateway RECEIVES  -> ResolutionChange (a change hash to apply on
//!                            the integration channel)
//!
//! Whether the operator works alone or with an LLM (e.g. through
//! mcp-server-pijul pointed at the gateway's working repository) changes
//! NOTHING here: both produce the same ResolutionChange through the same
//! queue. The gateway cannot tell them apart, and must not be able to.

use serde::{Deserialize, Serialize};

/// What the gateway shows for one unresolved conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReport {
    /// File path in conflict.
    pub path: String,
    /// The Pijul conflict state of the file (the gateway's own rendering).
    pub conflict_state: String,
    /// The PRs whose changes collide on this path.
    pub prs: Vec<PrContext>,
}

/// Minimal PR context shown alongside a conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrContext {
    pub number: u64,
    pub title: String,
    pub author: String,
}

/// What the operator sends back: a recorded change that resolves the
/// conflict. The gateway applies it and moves on. It never inspects how
/// the change was produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionChange {
    /// Hash (or unambiguous prefix) of the resolving change.
    pub change_hash: String,
    /// Free-form justification, archived in the ledger for audit.
    pub note: Option<String>,
}

/// The operator queue: exposed reports waiting for resolutions.
/// v0 is an in-process Vec; p2 turns it into the persistent queue behind the
/// CLI (and later web) console.
#[derive(Debug, Default)]
pub struct OperatorQueue {
    pub pending: Vec<ConflictReport>,
    pub received: Vec<ResolutionChange>,
}

impl OperatorQueue {
    pub fn submit(&mut self, resolution: ResolutionChange) {
        self.received.push(resolution);
        if !self.pending.is_empty() {
            self.pending.remove(0);
        }
    }
}


#[derive(Debug, Serialize)]
struct AuditEvent<'a> {
    event: &'static str,
    change_hash: &'a str,
    note: Option<&'a str>,
    recorded_at: String,
}

/// Apply an already available Pijul change to the integration channel and
/// append an audit trail. This does not generate the resolution change; the
/// operator or an assisting tool records it first in the same Pijul repo.
pub fn apply_resolution(
    workdir: &std::path::Path,
    change_hash: &str,
    note: Option<&str>,
) -> std::io::Result<()> {
    if change_hash.trim().is_empty()
        || !change_hash.chars().all(|character| character.is_ascii_alphanumeric())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "resolution change hash must be a non-empty alphanumeric hash or prefix",
        ));
    }
    std::fs::create_dir_all(workdir)?;
    let pijul_repo = workdir.join("pijul");
    let switch = std::process::Command::new("pijul")
        .args(["channel", "switch", "integration"])
        .current_dir(&pijul_repo)
        .output()?;
    if !switch.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "cannot switch to integration before applying resolution: {}",
                String::from_utf8_lossy(&switch.stderr).trim()
            ),
        ));
    }

    append_audit(workdir, "resolution-attempted", change_hash, note)?;
    let apply = std::process::Command::new("pijul")
        .args(["apply", change_hash])
        .current_dir(&pijul_repo)
        .output()?;
    if !apply.status.success() {
        let detail = String::from_utf8_lossy(&apply.stderr).trim().to_owned();
        let _ = append_audit(workdir, "resolution-failed", change_hash, note);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("pijul apply {change_hash} failed: {detail}"),
        ));
    }
    append_audit(workdir, "resolution-applied", change_hash, note)?;
    Ok(())
}

fn append_audit(
    workdir: &std::path::Path,
    event: &'static str,
    change_hash: &str,
    note: Option<&str>,
) -> std::io::Result<()> {
    use std::io::Write;
    let entry = AuditEvent {
        event,
        change_hash,
        note,
        recorded_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| format!("unix:{}", duration.as_secs()))
            .unwrap_or_else(|_| "unix:0".into()),
    };
    let serialized = serde_json::to_string(&entry)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(workdir.join("gateway-audit.jsonl"))?;
    writeln!(file, "{serialized}")
}


#[cfg(test)]
mod tests {
    use super::{append_audit, apply_resolution};
    use std::path::Path;

    #[test]
    fn audit_events_append_as_independent_json_lines() {
        let root = tempfile::tempdir().expect("temp root");
        append_audit(root.path(), "resolution-attempted", "ABC123", Some("keep both"))
            .expect("append first event");
        append_audit(root.path(), "resolution-applied", "ABC123", Some("keep both"))
            .expect("append second event");

        let raw = std::fs::read_to_string(root.path().join("gateway-audit.jsonl"))
            .expect("read audit");
        let lines: Vec<_> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("first JSON line");
        let second: serde_json::Value =
            serde_json::from_str(lines[1]).expect("second JSON line");
        assert_eq!(first["event"], "resolution-attempted");
        assert_eq!(second["event"], "resolution-applied");
        assert_eq!(second["note"], "keep both");
    }

    #[test]
    fn invalid_resolution_hash_is_rejected_before_running_pijul() {
        let error = apply_resolution(Path::new("/nonexistent-workdir"), "../outside", None)
            .expect_err("path-like input must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
