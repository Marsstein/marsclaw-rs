//! Skill management — installable prompt packs for MarsClaw.
//!
//! Ported from Go: internal/skills/skills.go

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write as IoWrite};
use std::path::PathBuf;

// ANSI escape codes.
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
}

/// Metadata for all built-in skills.
pub fn builtin_skills() -> Vec<Skill> {
    vec![
        Skill {
            id: "coder".into(),
            name: "Coder".into(),
            description: "Fast, precise coding assistant \u{2014} reads before editing, runs tests"
                .into(),
            source: "built-in".into(),
        },
        Skill {
            id: "devops".into(),
            name: "DevOps".into(),
            description: "Infrastructure, CI/CD, Docker, Kubernetes, cloud deployments".into(),
            source: "built-in".into(),
        },
        Skill {
            id: "writer".into(),
            name: "Writer".into(),
            description: "Technical writing, documentation, blog posts, clear communication".into(),
            source: "built-in".into(),
        },
        Skill {
            id: "analyst".into(),
            name: "Analyst".into(),
            description: "Data analysis, research, competitive intelligence, reports".into(),
            source: "built-in".into(),
        },
        Skill {
            id: "compliance".into(),
            name: "Compliance Officer".into(),
            description:
                "GDPR, ISO 27001, EU AI Act \u{2014} regulatory monitoring and gap analysis"
                    .into(),
            source: "built-in".into(),
        },
    ]
}

/// Built-in skill prompts mapped by ID.
pub fn builtin_prompts() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();

    m.insert(
        "coder",
        "You are MarsClaw, a fast and capable AI coding assistant.\n\n\
         Rules:\n\
         - Be concise and direct. Lead with the answer.\n\
         - Use tools to read files before editing them.\n\
         - Use edit_file for surgical changes, write_file for new files.\n\
         - Run shell commands to verify your work (tests, build).\n\
         - Never guess file contents \u{2014} always read first.\n\
         - When you're done, say what you did in 1-2 sentences.",
    );

    m.insert(
        "devops",
        "You are MarsClaw, a DevOps and infrastructure specialist.\n\n\
         Rules:\n\
         - Focus on reliability, security, and automation.\n\
         - Use infrastructure-as-code patterns (Terraform, Docker, k8s manifests).\n\
         - Always check current state before making changes (kubectl get, docker ps, etc).\n\
         - Prefer declarative over imperative approaches.\n\
         - Validate configurations before applying (--dry-run, terraform plan).\n\
         - Document any manual steps that can't be automated yet.",
    );

    m.insert(
        "writer",
        "You are MarsClaw, a technical writing assistant.\n\n\
         Rules:\n\
         - Write clearly and concisely. No filler words.\n\
         - Use active voice. Lead with the key point.\n\
         - Structure content with headers, bullets, and short paragraphs.\n\
         - Match the tone and style of existing documentation.\n\
         - Include code examples when explaining technical concepts.\n\
         - Proofread for clarity, not just grammar.",
    );

    m.insert(
        "analyst",
        "You are MarsClaw, a research and analysis assistant.\n\n\
         Rules:\n\
         - Start with the conclusion, then supporting evidence.\n\
         - Use data and specific numbers over vague claims.\n\
         - Compare alternatives with clear pros/cons.\n\
         - Cite sources when making factual claims.\n\
         - Flag assumptions and uncertainties explicitly.\n\
         - Present findings in structured tables when useful.",
    );

    m.insert(
        "compliance",
        "You are MarsClaw, a compliance and regulatory specialist for European regulations.\n\n\
         Rules:\n\
         - Reference specific articles and clauses (e.g. GDPR Art. 30, ISO 27001 A.8).\n\
         - Prioritize findings by risk level (critical, high, medium, low).\n\
         - Generate audit-ready documentation with proper formatting.\n\
         - Track regulatory changes and flag upcoming deadlines.\n\
         - Provide actionable remediation steps, not just findings.\n\
         - Maintain records of processing activities and DPIAs.",
    );

    m
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

pub fn skills_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".marsclaw")
        .join("skills")
}

pub fn active_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".marsclaw")
        .join("active_skill")
}

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// List all available skills (built-in + installed).
pub fn list_available() -> Vec<Skill> {
    let mut skills = builtin_skills();
    let prompts = builtin_prompts();

    for name in list_installed() {
        if prompts.contains_key(name.as_str()) {
            continue;
        }
        skills.push(Skill {
            id: name.clone(),
            name: name.clone(),
            description: "Custom installed skill".into(),
            source: "installed".into(),
        });
    }

    skills
}

/// List installed skill file names (without .md extension).
pub fn list_installed() -> Vec<String> {
    let dir = skills_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(stem) = name.strip_suffix(".md") {
            names.push(stem.to_string());
        }
    }
    names
}

/// Save a built-in skill prompt to disk.
pub fn install_builtin(id: &str) -> anyhow::Result<()> {
    let prompts = builtin_prompts();
    let prompt_text = prompts
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("unknown built-in skill: {id}"))?;

    let dir = skills_dir();
    fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{id}.md"));
    fs::write(path, prompt_text)?;
    Ok(())
}

/// Download and install a skill from a URL.
pub fn install_from_url(url: &str, name: &str) -> anyhow::Result<String> {
    let dir = skills_dir();
    fs::create_dir_all(&dir)?;

    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?
        .get(url)
        .send()?;

    if !resp.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", resp.status());
    }

    let body = resp.text()?;

    let safe: String = name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();

    let path = dir.join(format!("{safe}.md"));
    fs::write(path, &body)?;

    Ok(safe)
}

/// Mark a skill as active.
pub fn set_active(id: &str) -> anyhow::Result<()> {
    let path = active_file();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, id)?;
    Ok(())
}

/// Get the currently active skill ID.
pub fn get_active() -> Option<String> {
    fs::read_to_string(active_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get the prompt content for the active skill.
pub fn get_active_prompt() -> Option<String> {
    let id = get_active()?;

    let prompts = builtin_prompts();
    if let Some(prompt) = prompts.get(id.as_str()) {
        return Some(prompt.to_string());
    }

    let path = skills_dir().join(format!("{id}.md"));
    fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// CLI helpers
// ---------------------------------------------------------------------------

fn confirm_activate() -> bool {
    print!("  Set as active skill? [Y/n] ");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok();
    let trimmed = line.trim().to_lowercase();
    trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
}

// ---------------------------------------------------------------------------
// CLI commands
// ---------------------------------------------------------------------------

/// Show all available skills.
pub fn run_list() -> anyhow::Result<()> {
    let active = get_active();
    let installed = list_installed();
    let installed_set: std::collections::HashSet<&str> =
        installed.iter().map(|s| s.as_str()).collect();
    let prompts = builtin_prompts();

    println!("\n  {BOLD}MarsClaw Skills{RESET}\n");

    println!("  {CYAN}Built-in:{RESET}");
    for s in builtin_skills() {
        let marker = if active.as_deref() == Some(&s.id) {
            format!("{GREEN}\u{25cf}{RESET} ")
        } else {
            "  ".to_string()
        };
        println!("    {}{:<18} {}", marker, s.id, s.description);
    }

    let custom: Vec<&String> = installed
        .iter()
        .filter(|name| !prompts.contains_key(name.as_str()))
        .collect();

    if !custom.is_empty() {
        println!();
        println!("  {CYAN}Installed:{RESET}");
        for name in custom {
            let marker = if active.as_deref() == Some(name.as_str()) {
                format!("{GREEN}\u{25cf}{RESET} ")
            } else {
                "  ".to_string()
            };
            println!("    {}{}", marker, name);
        }
    }

    if let Some(ref id) = active {
        println!("\n  Active: {GREEN}{id}{RESET}");
    } else {
        println!("\n  No active skill (using SOUL.md or default)");
    }
    println!();

    // Suppress unused-variable warning for installed_set.
    let _ = installed_set;

    Ok(())
}

/// Install a skill by built-in ID or URL.
pub fn run_install(source: &str) -> anyhow::Result<()> {
    let prompts = builtin_prompts();

    if prompts.contains_key(source) {
        install_builtin(source)?;
        println!("  {GREEN}\u{2713} Installed built-in skill: {source}{RESET}");
        if confirm_activate() {
            set_active(source)?;
            println!("  {GREEN}\u{2713} Active skill set to: {source}{RESET}\n");
        }
        return Ok(());
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        print!("  Skill name: ");
        io::stdout().flush().ok();

        let stdin = io::stdin();
        let mut name = String::new();
        stdin.lock().read_line(&mut name).ok();
        let name = name.trim();
        let name = if name.is_empty() { "custom-skill" } else { name };

        println!("  Downloading from {source}...");
        let safe = install_from_url(source, name)?;
        println!("  {GREEN}\u{2713} Installed: {safe}{RESET}");
        if confirm_activate() {
            set_active(&safe)?;
            println!("  {GREEN}\u{2713} Active skill set to: {safe}{RESET}\n");
        }
        return Ok(());
    }

    anyhow::bail!("unknown skill {source:?} \u{2014} use a built-in ID or URL")
}

/// Set the active skill.
pub fn run_use(id: &str) -> anyhow::Result<()> {
    let prompts = builtin_prompts();

    if !prompts.contains_key(id) {
        let path = skills_dir().join(format!("{id}.md"));
        if !path.exists() {
            anyhow::bail!("skill {id:?} not found \u{2014} run: marsclaw skills list");
        }
    }

    set_active(id)?;
    println!("  {GREEN}\u{2713} Active skill: {id}{RESET}\n");
    Ok(())
}
