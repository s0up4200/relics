use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8(output.stdout).ok()).flatten()
}

fn short_commit(commit: &str) -> Option<&str> {
    let commit = commit.trim();
    matches!(commit.len(), 40 | 64)
        .then_some(commit)
        .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| &s[..12])
}

fn revision() -> Option<String> {
    let root = std::env::current_dir().ok()?.canonicalize().ok()?;
    let in_checkout = git(&["rev-parse", "--show-toplevel"])
        .and_then(|path| Path::new(path.trim()).canonicalize().ok())
        .is_some_and(|path| path == root);
    if !in_checkout {
        return short_commit(include_str!(".git-revision")).map(str::to_owned);
    }

    // Watch Git's resolved paths so linked worktrees and commits on the same branch stay current.
    for name in ["HEAD", "index", "packed-refs"] {
        let path = git(&["rev-parse", "--git-path", name])?;
        if Path::new(path.trim()).exists() {
            println!("cargo::rerun-if-changed={}", path.trim());
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let path = git(&["rev-parse", "--git-path", reference.trim()])?;
        let mut path = Path::new(path.trim());
        while !path.exists() {
            path = path.parent()?;
        }
        println!("cargo::rerun-if-changed={}", path.display());
    }
    for path in git(&["ls-files", "-z"])?.split('\0').filter(|path| !path.is_empty()) {
        println!("cargo::rerun-if-changed={path}");
    }
    let commit = git(&["rev-parse", "HEAD"])?;
    let commit = short_commit(&commit)?;
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty();
    Some(format!("{commit}{}", if dirty { "-dirty" } else { "" }))
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=.git-revision");
    let revision = revision().unwrap_or_else(|| {
        println!("cargo::warning=source commit is unavailable; build identity will show unknown");
        "unknown".into()
    });
    println!("cargo::rustc-env=RELICS_REVISION={revision}");
}

#[cfg(test)]
mod tests {
    use super::short_commit;

    #[test]
    fn archive_commit_is_validated_before_it_is_shortened() {
        assert_eq!(short_commit("0123456789abcdef0123456789abcdef01234567\n"), Some("0123456789ab"));
        assert_eq!(short_commit(&"a".repeat(64)), Some("aaaaaaaaaaaa"));
        for value in ["", "$Format:%H$", "abcdef123456", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            assert_eq!(short_commit(value), None);
        }
    }
}
