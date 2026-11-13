// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Read-only checkout preview: merge-tree writes Git objects, never the index or worktree.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, bail};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let upstream = match args {
        [] => "upstream/main",
        [reference] => reference,
        _ => bail!("usage: cargo xtask upstream-drift [REF]"),
    };
    println!("{}", report(&crate::gates::root(), upstream)?);
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> anyhow::Result<Output> {
    Command::new("git").current_dir(root).args(args).output().context("start git")
}

fn checked(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = git(root, args)?;
    if !output.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    }
    String::from_utf8(output.stdout).context("git output is not UTF-8")
}

fn revision(root: &Path, reference: &str) -> anyhow::Result<String> {
    let revision = checked(root, &["rev-parse", "--verify", "--end-of-options", &format!("{reference}^{{commit}}")])?;
    Ok(revision.trim().to_owned())
}

fn report(root: &Path, upstream: &str) -> anyhow::Result<String> {
    let head = revision(root, "HEAD")?;
    let upstream = revision(root, upstream)?;
    let range = format!("{head}...{upstream}");
    let [ahead, behind] = counts(root, &["rev-list", "--left-right", "--count", &range])?;
    // Upstream merges each pull request as one first-parent commit, so this count tracks PRs
    // and direct commits; the count above includes every commit those merges bring in.
    let [ahead_first, behind_first] = counts(root, &["rev-list", "--left-right", "--count", "--first-parent", &range])?;
    let merged = git(root, &["merge-tree", "--write-tree", "--name-only", "--no-messages", "-z", &head, &upstream])?;
    if !matches!(merged.status.code(), Some(0 | 1)) {
        bail!("git merge-tree: {}", String::from_utf8_lossy(&merged.stderr).trim());
    }
    let paths = conflict_paths(&merged.stdout)?;
    if merged.status.success() && !paths.is_empty() {
        bail!("git merge-tree status disagrees with its conflict paths");
    }
    let mut report = format!(
        "## Upstream drift\n\nFork: `{head}`\n\nUpstream: `{upstream}`\n\nAhead: {ahead} commits. Behind: {behind} commits.\n\nFirst-parent only (merged pull requests and direct commits): {ahead_first} ahead, {behind_first} behind.\n\nConflicted paths: {}.\n",
        paths.len()
    );
    if merged.status.success() {
        report.push_str("\nThe merge preview has no conflicts. This does not establish build or test compatibility.\n");
    } else {
        report.push_str("\nGit reports merge conflicts. The unmerged file list can be empty for some directory conflicts.\n\n");
        for path in &paths {
            // JSON quotes controls/newlines; encode Markdown/HTML delimiters too. Git paths are
            // upstream-controlled data, including when rendered in the standing issue.
            let display = match std::str::from_utf8(path) {
                Ok(path) => std::borrow::Cow::Borrowed(path),
                Err(_) => std::borrow::Cow::Owned(path.iter().map(|byte| format!("\\x{byte:02x}")).collect::<String>()),
            };
            let escaped =
                serde_json::to_string(&display)?.replace('`', "\\u0060").replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026");
            // Leave room under GitHub's issue body limit for the footer and marker.
            if report.len() + escaped.len() > 60_000 {
                report.push_str("\nPath list truncated. Use `git merge-tree --write-tree HEAD upstream/main` for the full list.\n");
                break;
            }
            report.push_str(&format!("- `{escaped}`\n"));
        }
    }
    report.push_str("\nPreview only. Merge through a reviewed PR after `cargo xtask fork-check` and the required checks pass.\n");
    Ok(report)
}

fn counts(root: &Path, args: &[&str]) -> anyhow::Result<[usize; 2]> {
    let output = checked(root, args)?;
    let counts: Vec<usize> = output.split_whitespace().map(str::parse).collect::<Result<_, _>>()?;
    let [left, right] = counts.as_slice() else { bail!("git rev-list did not return 2 counts") };
    Ok([*left, *right])
}

fn conflict_paths(output: &[u8]) -> anyhow::Result<BTreeSet<&[u8]>> {
    let mut fields = output.split(|byte| *byte == 0);
    let tree = fields.next().context("git merge-tree omitted its tree")?;
    if !matches!(tree.len(), 40 | 64) || !tree.iter().all(u8::is_ascii_hexdigit) {
        bail!("git merge-tree returned an invalid tree id");
    }
    Ok(fields.filter(|field| !field.is_empty()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("plutarch-drift-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&dir).unwrap();
            let repo = Self(dir);
            repo.command(&["init", "-q", "-b", "main"]);
            repo.command(&["config", "user.name", "Drift fixture"]);
            repo.command(&["config", "user.email", "fixture@example.invalid"]);
            repo.commit("croft-manor.txt", "base\n");
            repo.command(&["branch", "upstream"]);
            repo
        }
        fn command(&self, args: &[&str]) {
            checked(&self.0, args).unwrap();
        }
        fn commit(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(name), text).unwrap();
            self.command(&["add", "--", name]);
            self.command(&["commit", "-qm", "fixture"]);
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn counts_clean_divergence_without_changing_the_checkout() {
        let repo = Repo::new();
        repo.commit("fork.txt", "fork\n");
        repo.command(&["checkout", "-q", "upstream"]);
        repo.commit("upstream.txt", "upstream\n");
        repo.command(&["checkout", "-q", "main"]);
        let before = revision(&repo.0, "HEAD").unwrap();
        let report = report(&repo.0, "upstream").unwrap();
        assert!(report.contains("Ahead: 1 commits. Behind: 1 commits."), "{report}");
        assert!(report.contains("Conflicted paths: 0."), "{report}");
        assert_eq!(before, revision(&repo.0, "HEAD").unwrap());
        assert_eq!(checked(&repo.0, &["status", "--porcelain"]).unwrap(), "");
    }

    #[test]
    fn counts_a_merged_pull_request_once_on_the_first_parent_line() {
        let repo = Repo::new();
        repo.commit("fork.txt", "fork\n");
        repo.command(&["checkout", "-q", "-b", "feature", "upstream"]);
        repo.commit("feature-a.txt", "a\n");
        repo.commit("feature-b.txt", "b\n");
        repo.command(&["checkout", "-q", "upstream"]);
        repo.command(&["merge", "-q", "--no-ff", "-m", "Merge pull request", "feature"]);
        repo.command(&["checkout", "-q", "main"]);
        let report = report(&repo.0, "upstream").unwrap();
        assert!(report.contains("Ahead: 1 commits. Behind: 3 commits."), "{report}");
        assert!(report.contains("First-parent only (merged pull requests and direct commits): 1 ahead, 1 behind."), "{report}");
    }

    #[test]
    fn reports_conflicts_instead_of_treating_exit_one_as_failure() {
        let repo = Repo::new();
        repo.commit("croft-manor.txt", "fork\n");
        repo.command(&["checkout", "-q", "upstream"]);
        repo.commit("croft-manor.txt", "upstream\n");
        repo.command(&["checkout", "-q", "main"]);
        let report = report(&repo.0, "upstream").unwrap();
        assert!(report.contains("Conflicted paths: 1."), "{report}");
        assert!(report.contains("`\"croft-manor.txt\"`"), "{report}");
        assert_eq!(checked(&repo.0, &["status", "--porcelain"]).unwrap(), "");
    }

    #[test]
    fn missing_or_unrelated_ref_is_an_error() {
        let repo = Repo::new();
        assert!(report(&repo.0, "missing").is_err());
        repo.command(&["checkout", "--orphan", "unrelated"]);
        repo.command(&["commit", "-qm", "unrelated"]);
        assert!(report(&repo.0, "main").is_err());
    }

    #[test]
    fn nul_paths_keep_whitespace_and_reject_invalid_output() {
        let tree = "a".repeat(40);
        let output = format!("{tree}\0with spaces\0line\nbreak\0with spaces\0");
        assert_eq!(conflict_paths(output.as_bytes()).unwrap(), BTreeSet::from([b"line\nbreak".as_slice(), b"with spaces".as_slice()]));
        assert!(conflict_paths(b"not a tree\0path\0").is_err());
        let mut invalid = format!("{tree}\0").into_bytes();
        invalid.extend_from_slice(&[255, 0]);
        assert_eq!(conflict_paths(&invalid).unwrap(), BTreeSet::from([[255].as_slice()]));
    }
}
