//! Pure tests for the Git commit -> Pijul change mapping invariant.

use passerelle_git_pijul::translate::unique_new_change;
use std::collections::BTreeSet;

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn identifies_exactly_one_change_added_by_a_commit() {
    let parent = set(&["base-a", "base-b"]);
    let child = set(&["base-a", "base-b", "feature-c"]);
    assert_eq!(
        unique_new_change(&parent, &child, "git-feature").unwrap(),
        "feature-c"
    );
}

#[test]
fn rejects_a_commit_state_with_no_new_change() {
    let state = set(&["base-a", "base-b"]);
    let error = unique_new_change(&state, &state, "git-noop").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("found 0"));
}

#[test]
fn rejects_a_commit_state_with_multiple_new_changes() {
    let parent = set(&["base-a"]);
    let child = set(&["base-a", "feature-b", "feature-c"]);
    let error = unique_new_change(&parent, &child, "git-merge").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("found 2"));
}
