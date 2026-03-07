use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use aivyx_core::{AivyxError, Result};

/// Manages the `~/.aivyx/` directory layout.
///
/// ```text
/// ~/.aivyx/
/// ├── config.toml
/// ├── store.db
/// ├── audit.log
/// ├── agents/
/// ├── teams/
/// ├── sessions/
/// ├── memory/
/// ├── tasks/
/// ├── schedules/
/// ├── skills/
/// │   └── <name>/SKILL.md
/// ├── roles/
/// │   └── <role>.toml
/// ├── team-sessions/
/// └── keys/
///     └── master.json
/// ```
#[derive(Clone)]
pub struct AivyxDirs {
    root: PathBuf,
}

impl AivyxDirs {
    /// Resolve the aivyx data directory. Uses `~/.aivyx/` by default.
    pub fn default_root() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| AivyxError::Config("could not determine home directory".into()))?;
        Ok(home.join(".aivyx"))
    }

    /// Create an `AivyxDirs` for the given root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Create an `AivyxDirs` using the default `~/.aivyx/` location.
    pub fn from_default() -> Result<Self> {
        Ok(Self::new(Self::default_root()?))
    }

    /// Create all directories with 0o700 permissions.
    pub fn ensure_dirs(&self) -> Result<()> {
        create_dir_restricted(&self.root)?;
        create_dir_restricted(&self.keys_dir())?;
        create_dir_restricted(&self.agents_dir())?;
        create_dir_restricted(&self.teams_dir())?;
        create_dir_restricted(&self.sessions_dir())?;
        create_dir_restricted(&self.memory_dir())?;
        create_dir_restricted(&self.tasks_dir())?;
        create_dir_restricted(&self.schedules_dir())?;
        create_dir_restricted(&self.skills_dir())?;
        create_dir_restricted(&self.team_sessions_dir())?;
        create_dir_restricted(&self.roles_dir())?;
        Ok(())
    }

    /// Whether the root directory exists and contains a config.toml.
    pub fn is_initialized(&self) -> bool {
        self.config_path().exists()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn store_path(&self) -> PathBuf {
        self.root.join("store.db")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.root.join("audit.log")
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    pub fn master_key_path(&self) -> PathBuf {
        self.keys_dir().join("master.json")
    }

    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    pub fn teams_dir(&self) -> PathBuf {
        self.root.join("teams")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    /// Returns the path to the tasks directory (`~/.aivyx/tasks/`).
    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    /// Returns the path to the schedules directory (`~/.aivyx/schedules/`).
    pub fn schedules_dir(&self) -> PathBuf {
        self.root.join("schedules")
    }

    /// Returns the path to the skills directory (`~/.aivyx/skills/`).
    ///
    /// Skills are SKILL.md files following the
    /// [Agent Skills specification](https://agentskills.io/specification).
    /// Each skill lives in a subdirectory: `~/.aivyx/skills/<name>/SKILL.md`.
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// Returns the path to the roles directory (`~/.aivyx/roles/`).
    ///
    /// Role template TOML files placed here override the hardcoded role presets.
    /// For example, `~/.aivyx/roles/researcher.toml` overrides the built-in
    /// researcher profile when creating an agent with role "researcher".
    pub fn roles_dir(&self) -> PathBuf {
        self.root.join("roles")
    }

    /// Returns the path to the team sessions directory (`~/.aivyx/team-sessions/`).
    ///
    /// Team sessions persist lead and specialist conversation histories
    /// for cross-run resume.
    pub fn team_sessions_dir(&self) -> PathBuf {
        self.root.join("team-sessions")
    }
}

fn create_dir_restricted(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    // Best-effort: in Docker containers with bind mounts, the container user
    // may not own the directory and chmod will fail with EPERM. This is safe
    // to ignore — the directory was likely created with adequate permissions
    // by the entrypoint or Docker runtime.
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o700)) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!(
                "  [warn] could not set permissions on {}: {e} (ok in Docker)",
                path.display()
            );
        } else {
            return Err(e.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_correct() {
        let dirs = AivyxDirs::new("/tmp/test-aivyx");
        assert_eq!(
            dirs.config_path(),
            PathBuf::from("/tmp/test-aivyx/config.toml")
        );
        assert_eq!(dirs.store_path(), PathBuf::from("/tmp/test-aivyx/store.db"));
        assert_eq!(
            dirs.audit_path(),
            PathBuf::from("/tmp/test-aivyx/audit.log")
        );
        assert_eq!(
            dirs.master_key_path(),
            PathBuf::from("/tmp/test-aivyx/keys/master.json")
        );
        assert_eq!(dirs.agents_dir(), PathBuf::from("/tmp/test-aivyx/agents"));
        assert_eq!(dirs.teams_dir(), PathBuf::from("/tmp/test-aivyx/teams"));
        assert_eq!(
            dirs.sessions_dir(),
            PathBuf::from("/tmp/test-aivyx/sessions")
        );
        assert_eq!(dirs.memory_dir(), PathBuf::from("/tmp/test-aivyx/memory"));
        assert_eq!(dirs.tasks_dir(), PathBuf::from("/tmp/test-aivyx/tasks"));
        assert_eq!(
            dirs.schedules_dir(),
            PathBuf::from("/tmp/test-aivyx/schedules")
        );
        assert_eq!(dirs.skills_dir(), PathBuf::from("/tmp/test-aivyx/skills"));
        assert_eq!(
            dirs.team_sessions_dir(),
            PathBuf::from("/tmp/test-aivyx/team-sessions")
        );
        assert_eq!(dirs.roles_dir(), PathBuf::from("/tmp/test-aivyx/roles"));
    }

    #[test]
    fn ensure_dirs_creates_with_permissions() {
        let root = std::env::temp_dir().join(format!("aivyx-dirs-test-{}", rand::random::<u64>()));
        let dirs = AivyxDirs::new(&root);
        dirs.ensure_dirs().unwrap();

        assert!(root.exists());
        assert!(dirs.keys_dir().exists());

        let meta = fs::metadata(&root).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);

        let keys_meta = fs::metadata(dirs.keys_dir()).unwrap();
        assert_eq!(keys_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.agents_dir().exists());
        assert!(dirs.teams_dir().exists());
        assert!(dirs.sessions_dir().exists());
        let agents_meta = fs::metadata(dirs.agents_dir()).unwrap();
        assert_eq!(agents_meta.permissions().mode() & 0o777, 0o700);
        let teams_meta = fs::metadata(dirs.teams_dir()).unwrap();
        assert_eq!(teams_meta.permissions().mode() & 0o777, 0o700);
        let sessions_meta = fs::metadata(dirs.sessions_dir()).unwrap();
        assert_eq!(sessions_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.memory_dir().exists());
        let memory_meta = fs::metadata(dirs.memory_dir()).unwrap();
        assert_eq!(memory_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.tasks_dir().exists());
        let tasks_meta = fs::metadata(dirs.tasks_dir()).unwrap();
        assert_eq!(tasks_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.schedules_dir().exists());
        let schedules_meta = fs::metadata(dirs.schedules_dir()).unwrap();
        assert_eq!(schedules_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.skills_dir().exists());
        let skills_meta = fs::metadata(dirs.skills_dir()).unwrap();
        assert_eq!(skills_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.team_sessions_dir().exists());
        let team_sessions_meta = fs::metadata(dirs.team_sessions_dir()).unwrap();
        assert_eq!(team_sessions_meta.permissions().mode() & 0o777, 0o700);

        assert!(dirs.roles_dir().exists());
        let roles_meta = fs::metadata(dirs.roles_dir()).unwrap();
        assert_eq!(roles_meta.permissions().mode() & 0o777, 0o700);

        fs::remove_dir_all(&root).ok();
    }
}
