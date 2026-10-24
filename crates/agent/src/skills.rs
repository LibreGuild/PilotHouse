//! Agent skills: folders with a `SKILL.md` (a name, a description, then
//! instructions) that Claude Code and Codex load when a task needs them.
//! PilotHouse lists them and starts new ones; the agents do the rest.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Where a skill lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// In the project (`.claude/skills`), shared with everyone on it.
    Project,
    /// In the user's home, for every project.
    Personal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The agent that reads it.
    pub agent: &'static str,
    pub scope: Scope,
    /// Its `SKILL.md`.
    pub path: PathBuf,
}

/// The skill folders each agent reads.
fn roots(home: &Path, project: &Path) -> [(&'static str, Scope, PathBuf); 3] {
    [
        (
            "Claude Code",
            Scope::Project,
            project.join(".claude/skills"),
        ),
        ("Claude Code", Scope::Personal, home.join(".claude/skills")),
        ("Codex", Scope::Personal, home.join(".codex/skills")),
    ]
}

/// Every skill found, project ones first, then by name.
pub fn discover(home: &Path, project: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    for (agent, scope, root) in roots(home, project) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut found: Vec<Skill> = entries
            .flatten()
            .map(|entry| entry.path().join("SKILL.md"))
            .filter(|path| path.is_file())
            .map(|path| {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let (name, description) = front_matter(&text);
                let folder = path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Skill {
                    name: name.unwrap_or(folder),
                    description: description.unwrap_or_default(),
                    agent,
                    scope,
                    path,
                }
            })
            .collect();
        found.sort_by_key(|skill| skill.name.to_lowercase());
        skills.extend(found);
    }
    skills
}

/// The `name:` and `description:` of a `---` block at the top.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).to_string();
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    (name, description)
}

/// Starts a project skill: `.claude/skills/<name>/SKILL.md` with the parts
/// to fill in. Returns its path.
pub fn create(project: &Path, name: &str, description: &str) -> Result<PathBuf> {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        bail!("Give the skill a name.");
    }
    let folder = project.join(".claude/skills").join(&slug);
    let path = folder.join("SKILL.md");
    if path.exists() {
        bail!("There is already a skill named “{slug}”.");
    }
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    let description = match description.trim() {
        "" => "What this skill does, and when to use it.",
        text => text,
    };
    std::fs::write(
        &path,
        format!(
            "---\nname: {slug}\ndescription: {description}\n---\n\n# {name}\n\nSteps the agent should follow, \
             with the commands and files they involve.\n",
            name = name.trim()
        ),
    )?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_are_found_with_their_front_matter() {
        let home = std::env::temp_dir().join(format!("ph-skills-{}", std::process::id()));
        let project = home.join("app");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".codex/skills/release")).unwrap();
        std::fs::write(
            home.join(".codex/skills/release/SKILL.md"),
            "---\nname: release\ndescription: \"Cut a release\"\n---\nSteps",
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".claude/skills/no-front")).unwrap();
        std::fs::write(home.join(".claude/skills/no-front/SKILL.md"), "# Just text").unwrap();
        std::fs::create_dir_all(home.join(".claude/skills/empty")).unwrap();

        let path = create(&project, "Run the Tests!", "").unwrap();
        assert!(path.ends_with(".claude/skills/run-the-tests/SKILL.md"));
        assert!(
            create(&project, "run the tests", "").is_err(),
            "the same name twice"
        );
        assert!(create(&project, "  !! ", "").is_err());

        let skills = discover(&home, &project);
        let found: Vec<_> = skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.agent, skill.scope))
            .collect();
        assert_eq!(
            found,
            [
                ("run-the-tests", "Claude Code", Scope::Project),
                ("no-front", "Claude Code", Scope::Personal),
                ("release", "Codex", Scope::Personal),
            ],
            "folders without SKILL.md are not skills"
        );
        assert_eq!(skills[2].description, "Cut a release");
        assert_eq!(
            skills[0].description,
            "What this skill does, and when to use it."
        );
    }
}
