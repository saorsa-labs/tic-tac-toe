use super::*;

// ── is_dev_data_dir_name predicate ──────────────────────────────────────────

#[test]
fn is_dev_data_dir_name_rejects_prod_identifier() {
    assert!(!is_dev_data_dir_name("com.saorsalabs.tictactoe"));
}

/// The legacy (pre-rename) dev identifier is a migration *source*, never the
/// canonical dev dir — treating it as dev would resurrect the old data dir.
#[test]
fn is_dev_data_dir_name_rejects_legacy_dev_identifier() {
    assert!(!is_dev_data_dir_name("xyz.block.buzz.app.dev"));
}

#[test]
fn is_dev_data_dir_name_accepts_canonical_dev_identifier() {
    assert!(is_dev_data_dir_name("com.saorsalabs.tictactoe.dev"));
}

#[test]
fn is_dev_data_dir_name_accepts_worktree_dev_identifier() {
    assert!(is_dev_data_dir_name(
        "com.saorsalabs.tictactoe.dev.some-worktree"
    ));
}

/// Prefix-collision guard: an identifier that merely starts with the dev
/// prefix but is not dot-separated must be treated as prod, not dev.
/// `com.saorsalabs.tictactoe.developer` is a hypothetical prod variant, not a
/// worktree of `com.saorsalabs.tictactoe.dev`.
#[test]
fn is_dev_data_dir_name_rejects_prefix_collision() {
    assert!(!is_dev_data_dir_name("com.saorsalabs.tictactoe.developer"));
}
