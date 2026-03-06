//! SKILL.md progressive loader for agent context injection.
//!
//! Implements the three-tier progressive disclosure model:
//! - **Tier 1 (Discovery)**: Name + description loaded at agent startup (~50 tokens/skill)
//! - **Tier 2 (Activation)**: Full SKILL.md body loaded on demand (500-5000 tokens/skill)
//! - **Tier 3 (Execution)**: Referenced files loaded from `scripts/`, `references/` (deferred)
//!
//! Skills inject into the **system prompt**, not the tool registry. They teach
//! the agent *how* to use tools it already has — they don't add new capabilities.

use std::collections::HashMap;
use std::path::PathBuf;

use aivyx_config::{LoadedSkill, SkillSummary, discover_skills, load_skill};
use aivyx_core::{AivyxError, Result};

/// Manages skill discovery and progressive loading for an agent.
///
/// Created during agent session setup. Scans skill directories for SKILL.md
/// files, builds Tier-1 summaries for system prompt injection, and caches
/// fully loaded skills (Tier 2) on demand.
pub struct SkillLoader {
    /// Tier-1 summaries (loaded at startup, injected into every system prompt).
    summaries: Vec<SkillSummary>,
    /// Cache of fully loaded skills (Tier 2, loaded on demand).
    loaded_cache: HashMap<String, LoadedSkill>,
}

impl SkillLoader {
    /// Create a new loader by scanning skill directories.
    ///
    /// Scans both user-global (`~/.aivyx/skills/`) and project-local
    /// (`.aivyx/skills/`) directories. When the same skill name exists in
    /// multiple directories, the **first** directory in the list wins
    /// (project-local should be listed first for override semantics).
    pub fn discover(dirs: &[PathBuf]) -> Result<Self> {
        let mut summaries = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for dir in dirs {
            if dir.exists() {
                let discovered = discover_skills(dir)?;
                for summary in discovered {
                    if seen_names.insert(summary.name.clone()) {
                        summaries.push(summary);
                    }
                }
            }
        }

        // Sort by name for deterministic prompt ordering
        summaries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Self {
            summaries,
            loaded_cache: HashMap::new(),
        })
    }

    /// Format Tier-1 block for system prompt injection.
    ///
    /// Returns `None` if no skills are installed. The block lists each
    /// skill's name and description, guiding the agent to activate
    /// relevant skills via the `skill_activate` tool.
    pub fn format_discovery_block(&self) -> Option<String> {
        if self.summaries.is_empty() {
            return None;
        }

        let mut block = String::from("[AVAILABLE SKILLS]\n");
        block.push_str(
            "The following skills are available. \
             To activate a skill, use the skill_activate tool with the skill name.\n\n",
        );
        for s in &self.summaries {
            block.push_str(&format!("- **{}**: {}\n", s.name, s.description));
        }
        block.push_str("[END AVAILABLE SKILLS]");
        Some(block)
    }

    /// Activate a skill by name (Tier 2 load). Caches the result.
    ///
    /// Returns the full skill body for system prompt injection or tool result.
    pub fn activate(&mut self, name: &str) -> Result<&LoadedSkill> {
        if !self.loaded_cache.contains_key(name) {
            let summary = self
                .summaries
                .iter()
                .find(|s| s.name == name)
                .ok_or_else(|| AivyxError::Config(format!("skill not found: {name}")))?;
            let skill = load_skill(&summary.path)?;
            self.loaded_cache.insert(name.to_string(), skill);
        }
        Ok(&self.loaded_cache[name])
    }

    /// Check if any skills are available.
    pub fn has_skills(&self) -> bool {
        !self.summaries.is_empty()
    }

    /// List available skill names.
    pub fn skill_names(&self) -> Vec<&str> {
        self.summaries.iter().map(|s| s.name.as_str()).collect()
    }

    /// Get the Tier-1 summaries.
    pub fn summaries(&self) -> &[SkillSummary] {
        &self.summaries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_id() -> u64 {
        COUNTER.fetch_add(1, Ordering::Relaxed) + std::process::id() as u64
    }

    fn create_skill(base: &Path, name: &str, desc: &str) {
        let skill_dir = base.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {desc}\n---\n# {}\n\nInstructions for {}.",
                name.to_uppercase(),
                name
            ),
        )
        .unwrap();
    }

    #[test]
    fn discover_empty_dir() {
        let dir = std::env::temp_dir().join(format!("aivyx-sl-empty-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();

        let loader = SkillLoader::discover(&[dir.clone()]).unwrap();
        assert!(!loader.has_skills());
        assert_eq!(loader.skill_names().len(), 0);
        assert!(loader.format_discovery_block().is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_valid_skills() {
        let dir = std::env::temp_dir().join(format!("aivyx-sl-valid-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();

        create_skill(&dir, "alpha-skill", "Alpha description");
        create_skill(&dir, "beta-skill", "Beta description");

        let loader = SkillLoader::discover(&[dir.clone()]).unwrap();
        assert!(loader.has_skills());
        assert_eq!(loader.skill_names(), vec!["alpha-skill", "beta-skill"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn activate_loads_full_body() {
        let dir = std::env::temp_dir().join(format!("aivyx-sl-act-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();

        create_skill(&dir, "test-skill", "Test description");

        let mut loader = SkillLoader::discover(&[dir.clone()]).unwrap();
        let loaded = loader.activate("test-skill").unwrap();
        assert_eq!(loaded.manifest.name, "test-skill");
        assert!(loaded.body.contains("Instructions for test-skill"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn activate_unknown_skill_errors() {
        let dir = std::env::temp_dir().join(format!("aivyx-sl-unk-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();

        let mut loader = SkillLoader::discover(&[dir.clone()]).unwrap();
        let result = loader.activate("nonexistent");
        assert!(result.is_err());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_discovery_block_content() {
        let dir = std::env::temp_dir().join(format!("aivyx-sl-fmt-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();

        create_skill(&dir, "web-testing", "Guide for web testing");
        create_skill(&dir, "code-review", "Automated code review");

        let loader = SkillLoader::discover(&[dir.clone()]).unwrap();
        let block = loader.format_discovery_block().unwrap();
        assert!(block.contains("[AVAILABLE SKILLS]"));
        assert!(block.contains("[END AVAILABLE SKILLS]"));
        assert!(block.contains("**code-review**"));
        assert!(block.contains("**web-testing**"));
        assert!(block.contains("skill_activate"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_local_overrides_global() {
        let global = std::env::temp_dir().join(format!("aivyx-sl-global-{}", unique_id()));
        let local = std::env::temp_dir().join(format!("aivyx-sl-local-{}", unique_id()));
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&local).unwrap();

        // Same skill name in both; local listed first should win
        create_skill(&global, "shared-skill", "Global version");
        create_skill(&local, "shared-skill", "Local version");
        create_skill(&global, "global-only", "Only in global");

        // Local first for override semantics
        let loader = SkillLoader::discover(&[local.clone(), global.clone()]).unwrap();
        assert_eq!(loader.skill_names().len(), 2);

        // Verify the local version won by checking the summary description
        let shared = loader
            .summaries()
            .iter()
            .find(|s| s.name == "shared-skill")
            .unwrap();
        // The path should be under local dir, not global
        assert!(shared.path.starts_with(&local));

        fs::remove_dir_all(&global).ok();
        fs::remove_dir_all(&local).ok();
    }

    #[test]
    fn nonexistent_dirs_are_skipped() {
        let real_dir = std::env::temp_dir().join(format!("aivyx-sl-real-{}", unique_id()));
        fs::create_dir_all(&real_dir).unwrap();
        create_skill(&real_dir, "real-skill", "A real skill");

        let fake_dir = PathBuf::from("/tmp/aivyx-nonexistent-skills-dir-42");

        let loader = SkillLoader::discover(&[fake_dir, real_dir.clone()]).unwrap();
        assert_eq!(loader.skill_names(), vec!["real-skill"]);

        fs::remove_dir_all(&real_dir).ok();
    }
}
