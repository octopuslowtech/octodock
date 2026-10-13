// Adapted from skills-manager, src-tauri/src/core/sync_engine.rs
// (https://github.com/xingkongliang/skills-manager).
// MIT License, Copyright (c) 2026 Tianliang Zhang.

use std::ffi::OsStr;
use std::fs::FileType;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use zeron_proto::skill_library::SkillSyncMode;

const MAX_COPIED_FILES: usize = 20_000;
const MAX_COPIED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetState {
    Absent,
    LinkToSource,
    ForeignLink,
    RealDir,
    RealFile,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ReplacePolicy {
    NoClobber,
    Recorded(SkillSyncMode),
    UserConfirmed,
}

impl ReplacePolicy {
    fn permits(self, state: TargetState) -> bool {
        match state {
            TargetState::Absent | TargetState::LinkToSource => true,
            TargetState::ForeignLink => matches!(
                self,
                Self::Recorded(SkillSyncMode::Symlink) | Self::UserConfirmed
            ),
            TargetState::RealDir => matches!(
                self,
                Self::Recorded(SkillSyncMode::Copy) | Self::UserConfirmed
            ),
            TargetState::RealFile => matches!(self, Self::UserConfirmed),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::NoClobber => "is not managed by the skill library",
            Self::Recorded(_) => "does not match its recorded deployment",
            Self::UserConfirmed => "cannot be replaced",
        }
    }
}

pub(crate) fn skipped(name: &OsStr, kind: FileType) -> bool {
    name == ".git" || name == ".DS_Store" || kind.is_symlink()
}

pub(crate) fn ensure_disjoint(source: &Path, target: &Path) -> Result<()> {
    if source == target {
        bail!("Source and destination are the same path {source:?}");
    }
    let source_canonical = source
        .canonicalize()
        .with_context(|| format!("Source {source:?} does not exist or is not accessible"))?;
    let target_is_link = std::fs::symlink_metadata(target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false);
    let lexical = |path: &Path| -> Option<PathBuf> {
        Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?))
    };
    let target_canonical = if target_is_link {
        lexical(target)
    } else {
        target.canonicalize().ok().or_else(|| lexical(target))
    };
    if target_is_link
        && let (Some(source), Some(target)) = (lexical(source), target_canonical.as_ref())
        && source == *target
    {
        bail!("Source and destination are the same path {source:?}");
    }
    if let Some(target_canonical) = target_canonical {
        if target_canonical.starts_with(&source_canonical) {
            bail!("Destination {target:?} is inside source {source:?}");
        }
        if source_canonical.starts_with(&target_canonical) {
            bail!("Source {source:?} is inside destination {target:?}");
        }
    }
    Ok(())
}

pub(crate) fn classify(target: &Path, source: Option<&Path>) -> Result<TargetState> {
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TargetState::Absent);
        }
        Err(error) => {
            return Err(anyhow::Error::from(error)
                .context(format!("Cannot inspect existing target {target:?}")));
        }
    };
    if metadata.file_type().is_symlink() {
        if source.is_some_and(|source| link_points_to(target, source)) {
            return Ok(TargetState::LinkToSource);
        }
        return Ok(TargetState::ForeignLink);
    }
    if metadata.is_dir() {
        Ok(TargetState::RealDir)
    } else {
        Ok(TargetState::RealFile)
    }
}

fn link_points_to(target: &Path, source: &Path) -> bool {
    let Ok(link) = std::fs::read_link(target) else {
        return false;
    };
    let resolved = if link.is_absolute() {
        link
    } else {
        target.parent().unwrap_or(Path::new("")).join(link)
    };
    if resolved == source {
        return true;
    }
    match (resolved.canonicalize(), source.canonicalize()) {
        (Ok(link), Ok(source)) => link == source,
        _ => false,
    }
}

fn authorize(source: &Path, target: &Path, policy: ReplacePolicy) -> Result<TargetState> {
    let state = classify(target, Some(source))?;
    if !policy.permits(state) {
        bail!(
            "Refusing to replace {target:?}: it {}. The existing content was left untouched.",
            policy.describe()
        );
    }
    Ok(state)
}

pub(crate) fn preflight(source: &Path, target: &Path, policy: ReplacePolicy) -> Result<()> {
    authorize(source, target, policy).map(drop)
}

pub(crate) fn deploy(
    source: &Path,
    target: &Path,
    mode: SkillSyncMode,
    policy: ReplacePolicy,
) -> Result<SkillSyncMode> {
    let state = authorize(source, target, policy)?;
    if mode == SkillSyncMode::Symlink && state == TargetState::LinkToSource {
        return Ok(mode);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory {parent:?}"))?;
    }
    ensure_disjoint(source, target)?;
    remove_classified(target, state)
        .with_context(|| format!("Failed to remove existing target {target:?}"))?;
    match mode {
        SkillSyncMode::Symlink => link_or_copy(source, target),
        SkillSyncMode::Copy => {
            copy_dir(source, target)?;
            Ok(SkillSyncMode::Copy)
        }
    }
}

#[cfg(unix)]
fn link_or_copy(source: &Path, target: &Path) -> Result<SkillSyncMode> {
    std::os::unix::fs::symlink(source, target)
        .with_context(|| format!("Failed to create symlink {target:?} -> {source:?}"))?;
    Ok(SkillSyncMode::Symlink)
}

#[cfg(windows)]
fn link_or_copy(source: &Path, target: &Path) -> Result<SkillSyncMode> {
    match std::os::windows::fs::symlink_dir(source, target) {
        Ok(()) => Ok(SkillSyncMode::Symlink),
        Err(error) => {
            tracing::warn!(target = %target.display(), %error, "symlink unavailable; copying skill instead");
            copy_dir(source, target)?;
            Ok(SkillSyncMode::Copy)
        }
    }
}

pub(crate) fn remove_classified(target: &Path, state: TargetState) -> Result<()> {
    match state {
        TargetState::Absent => Ok(()),
        TargetState::LinkToSource | TargetState::ForeignLink => remove_link(target),
        TargetState::RealFile => std::fs::remove_file(target).map_err(Into::into),
        TargetState::RealDir => std::fs::remove_dir_all(target).map_err(Into::into),
    }
}

pub(crate) fn remove_recorded(target: &Path, mode: SkillSyncMode) -> Result<bool> {
    let state = classify(target, None)?;
    if !ReplacePolicy::Recorded(mode).permits(state) {
        return Ok(false);
    }
    remove_classified(target, state)?;
    Ok(true)
}

#[cfg(windows)]
fn remove_link(target: &Path) -> Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let metadata = match std::fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_symlink() {
        bail!("Refusing to remove {target:?}: expected a link but found a real file or directory");
    }
    if metadata.file_type().is_symlink_dir() {
        std::fs::remove_dir(target)?;
    } else {
        std::fs::remove_file(target)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn remove_link(target: &Path) -> Result<()> {
    match std::fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    copy_tree(source, target, &mut 0, &mut 0)
}

fn copy_tree(source: &Path, target: &Path, files: &mut usize, bytes: &mut u64) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if skipped(&name, kind) {
            continue;
        }
        let destination = target.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination, files, bytes)?;
        } else if kind.is_file() {
            *files += 1;
            *bytes = bytes.saturating_add(entry.metadata()?.len());
            if *files > MAX_COPIED_FILES || *bytes > MAX_COPIED_BYTES {
                bail!(
                    "Skill exceeds the {MAX_COPIED_FILES} file or {} MiB limit",
                    MAX_COPIED_BYTES / (1024 * 1024)
                );
            }
            std::fs::copy(entry.path(), &destination)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn skill(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# Skill").unwrap();
        dir
    }

    #[test]
    fn replace_policy_decision_table() {
        use SkillSyncMode::{Copy, Symlink};
        use TargetState::*;
        let table = [
            (ReplacePolicy::NoClobber, [true, true, false, false, false]),
            (
                ReplacePolicy::Recorded(Symlink),
                [true, true, true, false, false],
            ),
            (
                ReplacePolicy::Recorded(Copy),
                [true, true, false, true, false],
            ),
            (ReplacePolicy::UserConfirmed, [true, true, true, true, true]),
        ];
        for (policy, expected) in table {
            let states = [Absent, LinkToSource, ForeignLink, RealDir, RealFile];
            for (state, expected) in states.into_iter().zip(expected) {
                assert_eq!(policy.permits(state), expected, "{policy:?} on {state:?}");
            }
        }
    }

    #[test]
    fn copy_creates_files_and_skips_git_metadata() {
        let root = tempdir().unwrap();
        let source = skill(root.path(), "source");
        fs::create_dir(source.join(".git")).unwrap();
        fs::write(source.join(".git/config"), "x").unwrap();
        fs::create_dir(source.join("scripts")).unwrap();
        fs::write(source.join("scripts/run.sh"), "echo").unwrap();
        let target = root.path().join("agent/skills/source");
        let mode = deploy(
            &source,
            &target,
            SkillSyncMode::Copy,
            ReplacePolicy::NoClobber,
        )
        .unwrap();
        assert_eq!(mode, SkillSyncMode::Copy);
        assert!(target.join("SKILL.md").is_file());
        assert!(target.join("scripts/run.sh").is_file());
        assert!(!target.join(".git").exists());
    }

    #[test]
    fn refuses_an_unmanaged_directory_and_keeps_its_content() {
        let root = tempdir().unwrap();
        let source = skill(root.path(), "source");
        let target = skill(root.path(), "target");
        fs::write(target.join("notes.md"), "mine").unwrap();
        for mode in [SkillSyncMode::Symlink, SkillSyncMode::Copy] {
            let error = deploy(&source, &target, mode, ReplacePolicy::NoClobber).unwrap_err();
            assert!(error.to_string().contains("not managed"), "{error}");
            assert_eq!(fs::read_to_string(target.join("notes.md")).unwrap(), "mine");
        }
    }

    #[test]
    fn recorded_copy_is_replaced_and_user_confirmation_overrides_ownership() {
        let root = tempdir().unwrap();
        let source = skill(root.path(), "source");
        let target = skill(root.path(), "target");
        fs::write(target.join("stale.md"), "old").unwrap();
        deploy(
            &source,
            &target,
            SkillSyncMode::Copy,
            ReplacePolicy::Recorded(SkillSyncMode::Copy),
        )
        .unwrap();
        assert!(!target.join("stale.md").exists());

        let file = root.path().join("file");
        fs::write(&file, "user").unwrap();
        assert!(
            deploy(
                &source,
                &file,
                SkillSyncMode::Copy,
                ReplacePolicy::NoClobber
            )
            .is_err()
        );
        deploy(
            &source,
            &file,
            SkillSyncMode::Copy,
            ReplacePolicy::UserConfirmed,
        )
        .unwrap();
        assert!(file.join("SKILL.md").is_file());
    }

    #[test]
    fn overlapping_source_and_target_are_refused() {
        let root = tempdir().unwrap();
        let source = skill(root.path(), "source");
        assert!(ensure_disjoint(&source, &source).is_err());
        assert!(ensure_disjoint(&source, &source.join("nested")).is_err());
        assert!(ensure_disjoint(&source.join("SKILL.md"), root.path()).is_err());
        assert!(ensure_disjoint(&root.path().join("missing"), &source).is_err());
        assert!(ensure_disjoint(&source, &root.path().join("sibling")).is_ok());
        let error = deploy(
            &source,
            &source.join("nested"),
            SkillSyncMode::Copy,
            ReplacePolicy::NoClobber,
        )
        .unwrap_err();
        assert!(error.to_string().contains("inside source"), "{error}");
    }

    #[cfg(unix)]
    mod links {
        use super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn symlink_deploy_is_idempotent_and_classified_as_ours() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let target = root.path().join("agent/source");
            for _ in 0..2 {
                let mode = deploy(
                    &source,
                    &target,
                    SkillSyncMode::Symlink,
                    ReplacePolicy::NoClobber,
                )
                .unwrap();
                assert_eq!(mode, SkillSyncMode::Symlink);
            }
            assert_eq!(fs::read_link(&target).unwrap(), source);
            assert_eq!(
                classify(&target, Some(&source)).unwrap(),
                TargetState::LinkToSource
            );
            assert_eq!(
                classify(&target, None).unwrap(),
                TargetState::ForeignLink,
                "a link is never ours without a source to compare against"
            );
        }

        #[test]
        fn foreign_link_is_refused_and_its_pointee_survives() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let theirs = skill(root.path(), "theirs");
            let target = root.path().join("target");
            symlink(&theirs, &target).unwrap();
            assert!(
                deploy(
                    &source,
                    &target,
                    SkillSyncMode::Symlink,
                    ReplacePolicy::NoClobber
                )
                .is_err()
            );
            assert!(
                deploy(
                    &source,
                    &target,
                    SkillSyncMode::Copy,
                    ReplacePolicy::Recorded(SkillSyncMode::Copy)
                )
                .is_err()
            );
            assert_eq!(fs::read_link(&target).unwrap(), theirs);
            deploy(
                &source,
                &target,
                SkillSyncMode::Symlink,
                ReplacePolicy::Recorded(SkillSyncMode::Symlink),
            )
            .unwrap();
            assert_eq!(fs::read_link(&target).unwrap(), source);
            assert!(theirs.join("SKILL.md").is_file());
        }

        #[test]
        fn a_directory_that_replaced_a_recorded_link_is_preserved() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let target = skill(root.path(), "target");
            fs::write(target.join("notes.md"), "mine").unwrap();
            assert!(
                deploy(
                    &source,
                    &target,
                    SkillSyncMode::Symlink,
                    ReplacePolicy::Recorded(SkillSyncMode::Symlink)
                )
                .is_err()
            );
            assert!(!remove_recorded(&target, SkillSyncMode::Symlink).unwrap());
            assert_eq!(fs::read_to_string(target.join("notes.md")).unwrap(), "mine");
            assert!(remove_recorded(&target, SkillSyncMode::Copy).unwrap());
            assert!(!target.exists());
        }

        #[test]
        fn switching_our_link_to_a_copy_keeps_the_source() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let target = root.path().join("target");
            deploy(
                &source,
                &target,
                SkillSyncMode::Symlink,
                ReplacePolicy::NoClobber,
            )
            .unwrap();
            deploy(
                &source,
                &target,
                SkillSyncMode::Copy,
                ReplacePolicy::Recorded(SkillSyncMode::Symlink),
            )
            .unwrap();
            assert!(!fs::symlink_metadata(&target).unwrap().is_symlink());
            assert!(target.join("SKILL.md").is_file());
            assert!(source.join("SKILL.md").is_file());
        }

        #[test]
        fn removing_a_recorded_link_never_follows_it() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let target = root.path().join("target");
            symlink(&source, &target).unwrap();
            assert!(remove_recorded(&target, SkillSyncMode::Symlink).unwrap());
            assert!(fs::symlink_metadata(&target).is_err());
            assert!(source.join("SKILL.md").is_file());
            assert!(remove_recorded(&target, SkillSyncMode::Symlink).unwrap());
        }

        #[test]
        fn copy_skips_links_inside_the_skill() {
            let root = tempdir().unwrap();
            let source = skill(root.path(), "source");
            let secret = root.path().join("secret");
            fs::write(&secret, "token").unwrap();
            symlink(&secret, source.join("leak")).unwrap();
            let target = root.path().join("target");
            copy_dir(&source, &target).unwrap();
            assert!(fs::symlink_metadata(target.join("leak")).is_err());
        }
    }
}
