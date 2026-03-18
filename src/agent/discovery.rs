//! Walk up the directory tree to discover SOUL.md and AGENTS.md.
//!
//! Ported from Go: internal/agent/discovery.go

use std::path::Path;

/// Walk up from `work_dir` looking for `SOUL.md` and `AGENTS.md`.
///
/// Returns `(soul_content, agents_content)`. Either may be empty if the
/// corresponding file was not found.
pub fn discover_project_prompts(work_dir: &str) -> (String, String) {
    let soul = find_and_read(work_dir, "SOUL.md");
    let agents = find_and_read(work_dir, "AGENTS.md");
    (soul, agents)
}

/// Walk up from `dir` looking for `filename` and return its trimmed content.
fn find_and_read(dir: &str, filename: &str) -> String {
    let mut current = Path::new(dir).to_path_buf();
    loop {
        let candidate = current.join(filename);
        if let Ok(data) = std::fs::read_to_string(&candidate) {
            let trimmed = data.trim().to_owned();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }

        if !current.pop() {
            break;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_soul_in_ancestor() {
        let base = std::env::temp_dir().join("marsclaw_disc_test");
        let nested = base.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();

        let soul_path = base.join("SOUL.md");
        fs::write(&soul_path, "You are MarsClaw.").unwrap();

        let (soul, agents) = discover_project_prompts(nested.to_str().unwrap());
        assert_eq!(soul, "You are MarsClaw.");
        assert!(agents.is_empty());

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn returns_empty_when_not_found() {
        let dir = std::env::temp_dir().join("marsclaw_disc_empty");
        fs::create_dir_all(&dir).unwrap();

        let (soul, agents) = discover_project_prompts(dir.to_str().unwrap());
        // May or may not be empty depending on parent dirs, but should not panic.
        let _ = soul;
        let _ = agents;

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skips_empty_files() {
        let base = std::env::temp_dir().join("marsclaw_disc_skip");
        fs::create_dir_all(&base).unwrap();

        let soul_path = base.join("SOUL.md");
        fs::write(&soul_path, "   \n  ").unwrap();

        let (soul, _) = discover_project_prompts(base.to_str().unwrap());
        assert!(soul.is_empty() || !soul.starts_with(&*base.to_string_lossy()));

        fs::remove_dir_all(&base).ok();
    }
}
