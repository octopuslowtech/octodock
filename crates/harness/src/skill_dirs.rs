use std::ffi::OsString;
use std::path::{Path, PathBuf};

use zeron_proto::HarnessId;

pub const SHARED_DIR: &str = ".agents/skills";

pub struct SkillMetadata {
    pub name: String,
    pub description: String,
}

pub fn home() -> PathBuf {
    crate::executable::home_or_current_dir()
}

pub fn project_dir(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::ClaudeCode => Some(".claude/skills"),
        HarnessId::Codex => Some(".codex/skills"),
        HarnessId::Cursor => Some(".cursor/skills"),
        HarnessId::Opencode => Some(".opencode/skills"),
        HarnessId::Grok => Some(".grok/skills"),
        HarnessId::Pi => Some(".pi/skills"),
        HarnessId::Antigravity => Some(".gemini/skills"),
        HarnessId::Hermes | HarnessId::Devin | HarnessId::Amp | HarnessId::Mock => None,
    }
}

pub fn global_dir(
    harness: HarnessId,
    home: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let configured = |variable: &str, fallback: &str| {
        env(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(fallback))
    };
    match harness {
        HarnessId::ClaudeCode => Some(configured("CLAUDE_CONFIG_DIR", ".claude").join("skills")),
        HarnessId::Opencode => {
            Some(configured("XDG_CONFIG_HOME", ".config").join("opencode/skills"))
        }
        HarnessId::Hermes => Some(configured("HERMES_HOME", ".hermes").join("skills")),
        HarnessId::Pi => Some(configured("PI_CODING_AGENT_DIR", ".pi/agent").join("skills")),
        HarnessId::Codex | HarnessId::Cursor | HarnessId::Grok | HarnessId::Antigravity => {
            project_dir(harness).map(|dir| home.join(dir))
        }
        HarnessId::Devin | HarnessId::Amp | HarnessId::Mock => None,
    }
}

pub fn metadata(skill_dir: &Path) -> Option<SkillMetadata> {
    let skill = crate::skills::read_skill(&skill_dir.join("SKILL.md")).ok()??;
    Some(SkillMetadata {
        name: skill.name,
        description: skill.description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_dirs_follow_provider_overrides_and_fall_back_to_home() {
        let home = Path::new("/home/user");
        let unset = |_: &str| None;
        assert_eq!(
            global_dir(HarnessId::ClaudeCode, home, &unset),
            Some(home.join(".claude/skills"))
        );
        assert_eq!(
            global_dir(HarnessId::Opencode, home, &unset),
            Some(home.join(".config/opencode/skills"))
        );
        assert_eq!(
            global_dir(HarnessId::Cursor, home, &unset),
            Some(home.join(".cursor/skills"))
        );
        assert_eq!(global_dir(HarnessId::Amp, home, &unset), None);
        let overridden =
            |name: &str| (name == "CLAUDE_CONFIG_DIR").then(|| OsString::from("/etc/claude"));
        assert_eq!(
            global_dir(HarnessId::ClaudeCode, home, &overridden),
            Some(PathBuf::from("/etc/claude/skills"))
        );
    }

    #[test]
    fn metadata_reads_frontmatter_and_falls_back_to_the_directory_name() {
        let root = tempfile::tempdir().unwrap();
        let named = root.path().join("folder");
        std::fs::create_dir(&named).unwrap();
        std::fs::write(
            named.join("SKILL.md"),
            "---\nname: review\ndescription: Reviews code\n---\nBody\n",
        )
        .unwrap();
        let found = metadata(&named).unwrap();
        assert_eq!(found.name, "review");
        assert_eq!(found.description, "Reviews code");

        let plain = root.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        std::fs::write(plain.join("SKILL.md"), "Just text\n").unwrap();
        assert_eq!(metadata(&plain).unwrap().name, "plain");
        assert!(metadata(&root.path().join("missing")).is_none());
    }
}
