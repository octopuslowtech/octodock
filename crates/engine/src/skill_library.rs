use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeron_harness::skill_dirs;
use zeron_proto::HarnessId;
use zeron_proto::skill_library::{
    LibrarySkill, SkillDeployment, SkillDeploymentState, SkillLibraryOp, SkillLibraryView,
    SkillPreset, SkillSource, SkillSyncMode, SkillTarget, UnmanagedSkill,
};

mod deploy;

use deploy::{ReplacePolicy, TargetState};

const HARNESSES: [HarnessId; 10] = [
    HarnessId::ClaudeCode,
    HarnessId::Codex,
    HarnessId::Cursor,
    HarnessId::Opencode,
    HarnessId::Grok,
    HarnessId::Hermes,
    HarnessId::Pi,
    HarnessId::Antigravity,
    HarnessId::Devin,
    HarnessId::Amp,
];
const MAX_SCAN_DEPTH: usize = 5;
const MAX_SCANNED_DIRS: usize = 20_000;
const MAX_SKILLS_PER_SOURCE: usize = 200;
const MAX_UNMANAGED_PER_TARGET: usize = 512;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const GIT_PROTOCOLS: &str = "https:http:ssh:git:file";

type Env = fn(&str) -> Option<OsString>;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Manifest {
    mode: SkillSyncMode,
    skills: BTreeMap<String, SkillRecord>,
    presets: Vec<SkillPreset>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillRecord {
    source: SkillSource,
    hash: String,
    #[serde(default)]
    update_available: bool,
    #[serde(default)]
    deployments: Vec<DeploymentRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentRecord {
    path: PathBuf,
    mode: SkillSyncMode,
}

struct Inner {
    root: PathBuf,
    home: PathBuf,
    env: Env,
    lock: Mutex<()>,
}

#[derive(Clone)]
pub struct SkillLibrary {
    inner: Arc<Inner>,
}

impl SkillLibrary {
    pub fn new(root: PathBuf) -> Self {
        Self::with_home(root, skill_dirs::home(), |name| std::env::var_os(name))
    }

    fn with_home(root: PathBuf, home: PathBuf, env: Env) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                home,
                env,
                lock: Mutex::new(()),
            }),
        }
    }

    pub async fn view(&self, project: Option<PathBuf>) -> Result<SkillLibraryView> {
        let this = self.clone();
        blocking(move || this.snapshot(project.as_deref())).await
    }

    pub async fn apply(
        &self,
        op: SkillLibraryOp,
        project: Option<PathBuf>,
    ) -> Result<SkillLibraryView> {
        match op {
            SkillLibraryOp::InstallGit { source } => {
                self.install_git(&source).await?;
            }
            SkillLibraryOp::CheckUpdates => self.check_updates().await?,
            SkillLibraryOp::Update { skill } => self.update(skill).await?,
            op => {
                let this = self.clone();
                let project = project.clone();
                blocking(move || this.apply_local(op, project.as_deref())).await?;
            }
        }
        self.view(project).await
    }

    fn apply_local(&self, op: SkillLibraryOp, project: Option<&Path>) -> Result<()> {
        match op {
            SkillLibraryOp::InstallPath { path } => {
                self.install_path(&self.resolve_path(&path)?).map(drop)
            }
            SkillLibraryOp::Remove { skill } => self.remove(&skill),
            SkillLibraryOp::Deploy { skill, harness } => {
                let dir = self.target_dir(harness, project)?;
                self.mutate(|manifest| self.deploy_in(manifest, &skill, &dir))
            }
            SkillLibraryOp::Undeploy { skill, harness } => {
                let dir = self.target_dir(harness, project)?;
                self.mutate(|manifest| self.undeploy_in(manifest, &skill, &dir))
            }
            SkillLibraryOp::Adopt { path } => self.adopt(&self.resolve_path(&path)?, project),
            SkillLibraryOp::SavePreset { id, name, skills } => self.save_preset(id, &name, skills),
            SkillLibraryOp::DeletePreset { id } => self.mutate(|manifest| {
                manifest.presets.retain(|preset| preset.id != id);
                Ok(())
            }),
            SkillLibraryOp::ApplyPreset {
                id,
                harness,
                enabled,
            } => self.apply_preset(&id, &self.target_dir(harness, project)?, enabled),
            SkillLibraryOp::SetMode { mode } => self.mutate(|manifest| {
                manifest.mode = mode;
                Ok(())
            }),
            SkillLibraryOp::InstallGit { .. }
            | SkillLibraryOp::CheckUpdates
            | SkillLibraryOp::Update { .. } => bail!("This skill operation needs the network"),
        }
    }

    fn skills_dir(&self) -> PathBuf {
        self.inner.root.join("skills")
    }

    fn skill_dir(&self, id: &str) -> PathBuf {
        self.skills_dir().join(id)
    }

    fn manifest_path(&self) -> PathBuf {
        self.inner.root.join("manifest.json")
    }

    fn load(&self) -> Result<Manifest> {
        match std::fs::read(self.manifest_path()) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("The skill library manifest is unreadable")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(error) => Err(error).context("The skill library manifest is unreadable"),
        }
    }

    fn save(&self, manifest: &Manifest) -> Result<()> {
        std::fs::create_dir_all(&self.inner.root)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.inner.root)?;
        serde_json::to_writer_pretty(&mut file, manifest)?;
        file.persist(self.manifest_path())
            .map_err(|error| error.error)?;
        Ok(())
    }

    fn mutate<T>(&self, change: impl FnOnce(&mut Manifest) -> Result<T>) -> Result<T> {
        let _guard = self
            .inner
            .lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut manifest = self.load()?;
        let outcome = change(&mut manifest);
        self.save(&manifest)?;
        outcome
    }

    fn resolve_path(&self, raw: &str) -> Result<PathBuf> {
        let raw = raw.trim();
        let path = match raw.strip_prefix("~/") {
            Some(rest) => self.inner.home.join(rest),
            None if raw == "~" => self.inner.home.clone(),
            None => PathBuf::from(raw),
        };
        if !path.is_absolute() {
            bail!("Skill paths must be absolute");
        }
        Ok(path)
    }

    fn target_dir(&self, harness: Option<HarnessId>, project: Option<&Path>) -> Result<PathBuf> {
        let unsupported = |harness: HarnessId| {
            anyhow!("{harness:?} has no skills directory of its own; use the shared target")
        };
        match (harness, project) {
            (None, None) => Ok(self.inner.home.join(skill_dirs::SHARED_DIR)),
            (None, Some(root)) => Ok(root.join(skill_dirs::SHARED_DIR)),
            (Some(harness), None) => {
                skill_dirs::global_dir(harness, &self.inner.home, &self.inner.env)
                    .ok_or_else(|| unsupported(harness))
            }
            (Some(harness), Some(root)) => skill_dirs::project_dir(harness)
                .map(|dir| root.join(dir))
                .ok_or_else(|| unsupported(harness)),
        }
    }

    fn targets(&self, project: Option<&Path>) -> Vec<(Option<HarnessId>, PathBuf)> {
        std::iter::once(None)
            .chain(HARNESSES.into_iter().map(Some))
            .filter_map(|harness| Some((harness, self.target_dir(harness, project).ok()?)))
            .collect()
    }

    fn snapshot(&self, project: Option<&Path>) -> Result<SkillLibraryView> {
        let manifest = self.load()?;
        let targets = self.targets(project);
        let skills = manifest
            .skills
            .iter()
            .map(|(id, record)| {
                let source = self.skill_dir(id);
                let metadata = skill_dirs::metadata(&source);
                let deployments = targets
                    .iter()
                    .filter_map(|(harness, dir)| {
                        let target = dir.join(id);
                        let recorded = record
                            .deployments
                            .iter()
                            .find(|deployment| deployment.path == target)
                            .map(|deployment| deployment.mode);
                        let state = deploy::classify(&target, Some(&source)).ok()?;
                        let (mode, state) = match (recorded, state) {
                            (_, TargetState::LinkToSource) => {
                                (SkillSyncMode::Symlink, SkillDeploymentState::Deployed)
                            }
                            (Some(SkillSyncMode::Copy), TargetState::RealDir) => {
                                (SkillSyncMode::Copy, SkillDeploymentState::Deployed)
                            }
                            (Some(mode), TargetState::Absent) => {
                                (mode, SkillDeploymentState::Missing)
                            }
                            (Some(mode), _) => (mode, SkillDeploymentState::Conflict),
                            (None, _) => return None,
                        };
                        Some(SkillDeployment {
                            harness: *harness,
                            mode,
                            state,
                        })
                    })
                    .collect();
                LibrarySkill {
                    id: id.clone(),
                    name: metadata
                        .as_ref()
                        .map_or_else(|| id.clone(), |metadata| metadata.name.clone()),
                    description: metadata
                        .map(|metadata| metadata.description)
                        .unwrap_or_default(),
                    source: record.source.clone(),
                    update_available: record.update_available,
                    deployments,
                }
            })
            .collect();
        Ok(SkillLibraryView {
            mode: manifest.mode,
            skills,
            unmanaged: self.unmanaged(&manifest, &targets),
            targets: targets
                .into_iter()
                .map(|(harness, dir)| SkillTarget {
                    harness,
                    available: harness.is_none()
                        || project.is_some()
                        || dir.parent().is_some_and(Path::is_dir),
                    path: display(&dir),
                })
                .collect(),
            presets: manifest.presets,
        })
    }

    fn unmanaged(
        &self,
        manifest: &Manifest,
        targets: &[(Option<HarnessId>, PathBuf)],
    ) -> Vec<UnmanagedSkill> {
        let recorded: HashSet<&Path> = manifest
            .skills
            .values()
            .flat_map(|record| &record.deployments)
            .map(|deployment| deployment.path.as_path())
            .collect();
        let library = self.skills_dir().canonicalize().ok();
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        for (harness, dir) in targets {
            let Ok(children) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = children
                .take(MAX_UNMANAGED_PER_TARGET)
                .flatten()
                .map(|child| child.path())
                .collect();
            paths.sort();
            for path in paths {
                if recorded.contains(path.as_path()) || !path.join("SKILL.md").is_file() {
                    continue;
                }
                let Ok(canonical) = path.canonicalize() else {
                    continue;
                };
                if library
                    .as_ref()
                    .is_some_and(|library| canonical.starts_with(library))
                    || !seen.insert(canonical)
                {
                    continue;
                }
                let metadata = skill_dirs::metadata(&path);
                found.push(UnmanagedSkill {
                    harness: *harness,
                    name: metadata
                        .as_ref()
                        .map(|metadata| metadata.name.clone())
                        .or_else(|| Some(path.file_name()?.to_string_lossy().into_owned()))
                        .unwrap_or_default(),
                    description: metadata
                        .map(|metadata| metadata.description)
                        .unwrap_or_default(),
                    path: display(&path),
                });
            }
        }
        found
    }

    fn install_path(&self, source: &Path) -> Result<Vec<String>> {
        if source.is_dir() {
            let dirs = find_skill_dirs(source)?;
            return self.mutate(|manifest| {
                dirs.iter()
                    .map(|dir| {
                        let source = SkillSource::Local { path: display(dir) };
                        self.install_dir(manifest, dir, source)
                    })
                    .collect()
            });
        }
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "zip" | "skill") {
            bail!("Unsupported skill source: expected a folder, a .zip, or a .skill file");
        }
        let scratch = tempfile::tempdir()?;
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let root = scratch.path().join(sanitize_id(&stem));
        extract_archive(source, &root)?;
        let dirs = find_skill_dirs(&root)?;
        self.mutate(|manifest| {
            dirs.iter()
                .map(|dir| {
                    let source = SkillSource::Archive {
                        path: display(source),
                    };
                    self.install_dir(manifest, dir, source)
                })
                .collect()
        })
    }

    fn install_dir(
        &self,
        manifest: &mut Manifest,
        dir: &Path,
        source: SkillSource,
    ) -> Result<String> {
        let name = skill_dirs::metadata(dir)
            .map(|metadata| metadata.name)
            .or_else(|| Some(dir.file_name()?.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let base = sanitize_id(&name);
        let hash = hash_dir(dir)?;
        let mut suffix = 1_u32;
        let id = loop {
            let candidate = if suffix == 1 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            };
            let reusable = manifest
                .skills
                .get(&candidate)
                .is_none_or(|record| record.hash == hash || same_origin(&record.source, &source));
            if reusable {
                break candidate;
            }
            suffix += 1;
        };
        self.replace_content(manifest, &id, dir, hash, source)?;
        Ok(id)
    }

    fn replace_content(
        &self,
        manifest: &mut Manifest,
        id: &str,
        dir: &Path,
        hash: String,
        source: SkillSource,
    ) -> Result<()> {
        let destination = self.skill_dir(id);
        let unchanged = destination.is_dir()
            && manifest
                .skills
                .get(id)
                .is_some_and(|record| record.hash == hash);
        if !unchanged {
            std::fs::create_dir_all(self.skills_dir())?;
            let staging = tempfile::Builder::new()
                .prefix(".staging-")
                .tempdir_in(self.skills_dir())?;
            let staged = staging.path().join("next");
            deploy::copy_dir(dir, &staged)?;
            if std::fs::symlink_metadata(&destination).is_ok() {
                std::fs::rename(&destination, staging.path().join("previous"))?;
            }
            std::fs::rename(&staged, &destination)?;
        }
        let record = manifest
            .skills
            .entry(id.to_owned())
            .or_insert_with(|| SkillRecord {
                source: source.clone(),
                hash: hash.clone(),
                update_available: false,
                deployments: Vec::new(),
            });
        record.source = source;
        record.hash = hash;
        record.update_available = false;
        if !unchanged {
            for deployment in &record.deployments {
                if deployment.mode != SkillSyncMode::Copy {
                    continue;
                }
                if let Err(error) = deploy::deploy(
                    &destination,
                    &deployment.path,
                    SkillSyncMode::Copy,
                    ReplacePolicy::Recorded(SkillSyncMode::Copy),
                ) {
                    tracing::warn!(path = %deployment.path.display(), %error, "skill copy not refreshed");
                }
            }
        }
        Ok(())
    }

    fn deploy_in(&self, manifest: &mut Manifest, id: &str, dir: &Path) -> Result<()> {
        let mode = manifest.mode;
        let record = manifest.skills.get_mut(id).ok_or_else(|| unknown(id))?;
        let target = dir.join(id);
        let actual = deploy::deploy(
            &self.skill_dir(id),
            &target,
            mode,
            policy_for(record, &target),
        )?;
        record
            .deployments
            .retain(|deployment| deployment.path != target);
        record.deployments.push(DeploymentRecord {
            path: target,
            mode: actual,
        });
        Ok(())
    }

    fn undeploy_in(&self, manifest: &mut Manifest, id: &str, dir: &Path) -> Result<()> {
        let record = manifest.skills.get_mut(id).ok_or_else(|| unknown(id))?;
        let target = dir.join(id);
        let recorded = record
            .deployments
            .iter()
            .position(|deployment| deployment.path == target);
        match recorded {
            Some(index) => {
                if !deploy::remove_recorded(&target, record.deployments[index].mode)? {
                    tracing::warn!(path = %target.display(), "skill target changed since deployment; left in place");
                }
                record.deployments.remove(index);
            }
            None => {
                let state = deploy::classify(&target, Some(&self.skill_dir(id)))?;
                if state == TargetState::LinkToSource {
                    deploy::remove_classified(&target, state)?;
                }
            }
        }
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.mutate(|manifest| {
            let record = manifest.skills.remove(id).ok_or_else(|| unknown(id))?;
            for deployment in &record.deployments {
                match deploy::remove_recorded(&deployment.path, deployment.mode) {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::warn!(path = %deployment.path.display(), "skill target changed since deployment; left in place");
                    }
                    Err(error) => {
                        tracing::warn!(path = %deployment.path.display(), %error, "skill target not removed");
                    }
                }
            }
            for preset in &mut manifest.presets {
                preset.skills.retain(|skill| skill != id);
            }
            match std::fs::remove_dir_all(self.skill_dir(id)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            }
        })
    }

    fn adopt(&self, path: &Path, project: Option<&Path>) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("{} is not inside a skills directory", path.display()))?;
        let known = self.targets(project).into_iter().any(|(_, dir)| {
            dir == parent
                || matches!(
                    (dir.canonicalize(), parent.canonicalize()),
                    (Ok(dir), Ok(parent)) if dir == parent
                )
        });
        if !known {
            bail!("{} is not inside a known skills directory", path.display());
        }
        if !path.join("SKILL.md").is_file() {
            bail!("{} has no SKILL.md", path.display());
        }
        self.mutate(|manifest| {
            let source = SkillSource::Local {
                path: display(path),
            };
            let id = self.install_dir(manifest, path, source)?;
            let stored = self.skill_dir(&id);
            if hash_dir(&stored)? != hash_dir(path)? {
                bail!("The library copy of {id} does not match its source; nothing was replaced");
            }
            let target = parent.join(&id);
            let mode = manifest.mode;
            let actual = deploy::deploy(&stored, &target, mode, ReplacePolicy::UserConfirmed)?;
            if target != path {
                let state = deploy::classify(path, Some(&stored))?;
                deploy::remove_classified(path, state)?;
            }
            let record = manifest.skills.get_mut(&id).ok_or_else(|| unknown(&id))?;
            record
                .deployments
                .retain(|deployment| deployment.path != target);
            record.deployments.push(DeploymentRecord {
                path: target,
                mode: actual,
            });
            Ok(())
        })
    }

    fn save_preset(&self, id: Option<String>, name: &str, skills: Vec<String>) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("A preset needs a name");
        }
        self.mutate(|manifest| {
            let mut kept = Vec::new();
            for skill in skills {
                if manifest.skills.contains_key(&skill) && !kept.contains(&skill) {
                    kept.push(skill);
                }
            }
            match id.and_then(|id| manifest.presets.iter_mut().find(|preset| preset.id == id)) {
                Some(preset) => {
                    preset.name = name.to_owned();
                    preset.skills = kept;
                }
                None => manifest.presets.push(SkillPreset {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.to_owned(),
                    skills: kept,
                }),
            }
            Ok(())
        })
    }

    fn apply_preset(&self, id: &str, dir: &Path, enabled: bool) -> Result<()> {
        self.mutate(|manifest| {
            let preset = manifest
                .presets
                .iter()
                .find(|preset| preset.id == id)
                .ok_or_else(|| anyhow!("Unknown preset {id}"))?;
            let skills: Vec<String> = preset
                .skills
                .iter()
                .filter(|skill| manifest.skills.contains_key(*skill))
                .cloned()
                .collect();
            if enabled {
                for skill in &skills {
                    let target = dir.join(skill);
                    deploy::preflight(
                        &self.skill_dir(skill),
                        &target,
                        policy_for(&manifest.skills[skill], &target),
                    )?;
                }
            }
            for skill in &skills {
                if enabled {
                    self.deploy_in(manifest, skill, dir)?;
                } else {
                    self.undeploy_in(manifest, skill, dir)?;
                }
            }
            Ok(())
        })
    }

    async fn install_git(&self, source: &str) -> Result<Vec<String>> {
        let spec = GitSpec::parse(source)?;
        let checkout = clone(&spec.url, spec.branch.as_deref()).await?;
        let this = self.clone();
        blocking(move || {
            let root = checkout.subtree(spec.subpath.as_deref())?;
            let mut dirs = find_skill_dirs(&root)?;
            if let Some(skill) = &spec.skill {
                dirs.retain(|dir| dir.file_name().is_some_and(|name| name == skill.as_str()));
                if dirs.is_empty() {
                    bail!("Skill {skill} was not found in {}", spec.url);
                }
            }
            this.mutate(|manifest| {
                dirs.iter()
                    .map(|dir| {
                        let source = checkout.source(&spec.url, spec.branch.clone(), dir);
                        this.install_dir(manifest, dir, source)
                    })
                    .collect()
            })
        })
        .await
    }

    async fn check_updates(&self) -> Result<()> {
        let this = self.clone();
        let manifest = blocking(move || this.load()).await?;
        let mut heads: HashMap<(String, Option<String>), Option<String>> = HashMap::new();
        let mut failure = None;
        for record in manifest.skills.values() {
            let SkillSource::Git { url, branch, .. } = &record.source else {
                continue;
            };
            let key = (url.clone(), branch.clone());
            if heads.contains_key(&key) {
                continue;
            }
            let head = match remote_head(url, branch.as_deref()).await {
                Ok(head) => Some(head),
                Err(error) => {
                    failure = Some(error);
                    None
                }
            };
            heads.insert(key, head);
        }
        let this = self.clone();
        blocking(move || {
            this.mutate(|manifest| {
                for record in manifest.skills.values_mut() {
                    match &record.source {
                        SkillSource::Git {
                            url,
                            branch,
                            revision,
                            ..
                        } => {
                            if let Some(Some(head)) = heads.get(&(url.clone(), branch.clone())) {
                                record.update_available = revision.as_ref() != Some(head);
                            }
                        }
                        SkillSource::Local { path } => {
                            if let Ok(hash) = hash_dir(Path::new(path)) {
                                record.update_available = hash != record.hash;
                            }
                        }
                        SkillSource::Archive { .. } => {}
                    }
                }
                Ok(())
            })
        })
        .await?;
        failure.map_or(Ok(()), Err)
    }

    async fn update(&self, id: String) -> Result<()> {
        let this = self.clone();
        let key = id.clone();
        let source = blocking(move || {
            let record = this.load()?.skills.remove(&key);
            record
                .map(|record| record.source)
                .ok_or_else(|| unknown(&key))
        })
        .await?;
        let this = self.clone();
        match source {
            SkillSource::Git {
                url,
                branch,
                subpath,
                ..
            } => {
                let checkout = clone(&url, branch.as_deref()).await?;
                blocking(move || {
                    let dir = checkout.subtree(subpath.as_deref())?;
                    let source = checkout.source(&url, branch, &dir);
                    this.refresh(&id, &dir, source)
                })
                .await
            }
            SkillSource::Local { path } => {
                blocking(move || {
                    let source = SkillSource::Local { path: path.clone() };
                    this.refresh(&id, Path::new(&path), source)
                })
                .await
            }
            SkillSource::Archive { path } => {
                blocking(move || this.install_path(Path::new(&path)).map(drop)).await
            }
        }
    }

    fn refresh(&self, id: &str, dir: &Path, source: SkillSource) -> Result<()> {
        if !dir.join("SKILL.md").is_file() {
            bail!("{} no longer contains a SKILL.md", dir.display());
        }
        let hash = hash_dir(dir)?;
        self.mutate(|manifest| {
            if !manifest.skills.contains_key(id) {
                return Err(unknown(id));
            }
            self.replace_content(manifest, id, dir, hash, source)
        })
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .context("The skill library task failed")?
}

fn unknown(id: &str) -> anyhow::Error {
    anyhow!("Unknown skill {id}")
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn policy_for(record: &SkillRecord, target: &Path) -> ReplacePolicy {
    record
        .deployments
        .iter()
        .find(|deployment| deployment.path == target)
        .map_or(ReplacePolicy::NoClobber, |deployment| {
            ReplacePolicy::Recorded(deployment.mode)
        })
}

fn same_origin(left: &SkillSource, right: &SkillSource) -> bool {
    match (left, right) {
        (SkillSource::Local { path: left }, SkillSource::Local { path: right })
        | (SkillSource::Archive { path: left }, SkillSource::Archive { path: right }) => {
            left == right
        }
        (
            SkillSource::Git {
                url: left_url,
                subpath: left_subpath,
                ..
            },
            SkillSource::Git {
                url: right_url,
                subpath: right_subpath,
                ..
            },
        ) => left_url == right_url && left_subpath == right_subpath,
        _ => false,
    }
}

fn sanitize_id(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|character| matches!(character, '-' | '.'));
    if trimmed.is_empty() {
        "skill".into()
    } else {
        trimmed.into()
    }
}

fn hash_dir(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_tree(dir, Path::new(""), &mut hasher)
        .with_context(|| format!("Cannot read skill at {}", dir.display()))?;
    Ok(crate::repos::hex(&hasher.finalize()))
}

fn hash_tree(dir: &Path, relative: &Path, hasher: &mut Sha256) -> Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if deploy::skipped(&name, kind) {
            continue;
        }
        let relative = relative.join(&name);
        if kind.is_dir() {
            hash_tree(&entry.path(), &relative, hasher)?;
        } else if kind.is_file() {
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update([0]);
            let length = std::io::copy(&mut std::fs::File::open(entry.path())?, hasher)?;
            hasher.update(length.to_le_bytes());
        }
    }
    Ok(())
}

fn find_skill_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut scanned = 0_usize;
    while let Some((dir, depth)) = pending.pop() {
        scanned += 1;
        if scanned > MAX_SCANNED_DIRS {
            bail!("{} is too large to scan for skills", root.display());
        }
        if dir.join("SKILL.md").is_file() {
            found.push(dir);
            if found.len() > MAX_SKILLS_PER_SOURCE {
                bail!(
                    "{} holds more than {MAX_SKILLS_PER_SOURCE} skills; pick a subfolder",
                    root.display()
                );
            }
            continue;
        }
        if depth == MAX_SCAN_DEPTH {
            continue;
        }
        let Ok(children) = std::fs::read_dir(&dir) else {
            continue;
        };
        for child in children.flatten() {
            let name = child.file_name();
            if name == ".git" || name == "node_modules" {
                continue;
            }
            if child.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push((child.path(), depth + 1));
            }
        }
    }
    if found.is_empty() {
        bail!("No SKILL.md was found in {}", root.display());
    }
    found.sort();
    Ok(found)
}

fn extract_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Cannot open {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("Not a valid zip archive")?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!("The archive has more than {MAX_ARCHIVE_ENTRIES} entries");
    }
    let mut extracted = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry.enclosed_name() else {
            bail!("The archive contains an unsafe entry path");
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
        {
            continue;
        }
        let output = destination.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&output)?;
        let budget = MAX_ARCHIVE_BYTES - extracted;
        extracted += std::io::copy(&mut std::io::Read::take(&mut entry, budget + 1), &mut file)?;
        if extracted > MAX_ARCHIVE_BYTES {
            bail!(
                "The archive expands beyond the {} MiB limit",
                MAX_ARCHIVE_BYTES / (1024 * 1024)
            );
        }
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct GitSpec {
    url: String,
    branch: Option<String>,
    subpath: Option<String>,
    skill: Option<String>,
}

impl GitSpec {
    fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() || input.starts_with('-') {
            bail!("Enter a Git URL or an owner/repo name");
        }
        let plain = |url: &str| Self {
            url: url.to_owned(),
            branch: None,
            subpath: None,
            skill: None,
        };
        if let Some(rest) = input.strip_prefix("https://github.com/") {
            let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
            if let [owner, repo, "tree", branch, subpath @ ..] = parts.as_slice() {
                return Ok(Self {
                    url: format!("https://github.com/{owner}/{repo}.git"),
                    branch: Some((*branch).to_owned()),
                    subpath: (!subpath.is_empty()).then(|| subpath.join("/")),
                    skill: None,
                });
            }
            return Ok(plain(input));
        }
        if input.contains("://") || input.starts_with("git@") {
            return Ok(plain(input));
        }
        let (repo, skill) = match input.split_once('@') {
            Some((repo, skill)) => (repo, Some(skill)),
            None => (input, None),
        };
        let segment = |part: &str| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        };
        match repo.split('/').collect::<Vec<_>>().as_slice() {
            [owner, name] if segment(owner) && segment(name) && skill.is_none_or(segment) => {
                Ok(Self {
                    url: format!("https://github.com/{owner}/{name}.git"),
                    branch: None,
                    subpath: None,
                    skill: skill.map(str::to_owned),
                })
            }
            _ => bail!("{input} is not a Git URL or an owner/repo name"),
        }
    }
}

struct Checkout {
    root: PathBuf,
    revision: String,
    _scratch: tempfile::TempDir,
}

impl Checkout {
    fn subtree(&self, subpath: Option<&str>) -> Result<PathBuf> {
        let Some(subpath) = subpath else {
            return Ok(self.root.clone());
        };
        let resolved = self
            .root
            .join(subpath)
            .canonicalize()
            .with_context(|| format!("{subpath} does not exist in the repository"))?;
        if !resolved.starts_with(&self.root) {
            bail!("{subpath} is outside the repository");
        }
        Ok(resolved)
    }

    fn source(&self, url: &str, branch: Option<String>, dir: &Path) -> SkillSource {
        let subpath = dir
            .strip_prefix(&self.root)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| relative.to_string_lossy().replace('\\', "/"));
        SkillSource::Git {
            url: url.to_owned(),
            branch,
            subpath,
            revision: Some(self.revision.clone()),
        }
    }
}

fn git_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x08000000);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", GIT_PROTOCOLS)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}

async fn run_git(mut command: tokio::process::Command) -> Result<String> {
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            anyhow!(
                "git did not finish within {} seconds",
                GIT_TIMEOUT.as_secs()
            )
        })?
        .context("git could not be started")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn clone(url: &str, branch: Option<&str>) -> Result<Checkout> {
    let scratch = tempfile::tempdir()?;
    let name = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default();
    let root = scratch
        .path()
        .join(sanitize_id(name.trim_end_matches(".git")));
    let mut command = git_command();
    command.args(["clone", "--depth", "1", "--quiet"]);
    if let Some(branch) = branch {
        command.arg(format!("--branch={branch}"));
    }
    command.arg("--").arg(url).arg(&root);
    run_git(command).await?;
    let mut command = git_command();
    command.arg("-C").arg(&root).args(["rev-parse", "HEAD"]);
    let revision = run_git(command).await?;
    Ok(Checkout {
        root: root.canonicalize()?,
        revision,
        _scratch: scratch,
    })
}

async fn remote_head(url: &str, branch: Option<&str>) -> Result<String> {
    let mut command = git_command();
    command
        .args(["ls-remote", "--"])
        .arg(url)
        .arg(branch.unwrap_or("HEAD"));
    let output = run_git(command).await?;
    output
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{url} did not report a revision"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_safe_directory_names() {
        assert_eq!(sanitize_id("review"), "review");
        assert_eq!(sanitize_id("plugin:review tool"), "plugin-review-tool");
        assert_eq!(sanitize_id("../.."), "skill");
        assert_eq!(sanitize_id(".hidden"), "hidden");
        assert_eq!(sanitize_id(""), "skill");
    }

    #[test]
    fn git_sources_accept_urls_shorthands_and_tree_links() {
        assert_eq!(
            GitSpec::parse("vercel-labs/agent-skills@react").unwrap(),
            GitSpec {
                url: "https://github.com/vercel-labs/agent-skills.git".into(),
                branch: None,
                subpath: None,
                skill: Some("react".into()),
            }
        );
        assert_eq!(
            GitSpec::parse("https://github.com/foo/bar/tree/main/skills/baz").unwrap(),
            GitSpec {
                url: "https://github.com/foo/bar.git".into(),
                branch: Some("main".into()),
                subpath: Some("skills/baz".into()),
                skill: None,
            }
        );
        assert_eq!(
            GitSpec::parse(" git@example.com:team/skills.git ")
                .unwrap()
                .url,
            "git@example.com:team/skills.git"
        );
        assert_eq!(
            GitSpec::parse("https://github.com/foo/bar").unwrap().url,
            "https://github.com/foo/bar"
        );
        for rejected in ["", "--upload-pack=x", "just-a-name", "a/b/c", "../x"] {
            assert!(GitSpec::parse(rejected).is_err(), "{rejected}");
        }
    }

    #[cfg(unix)]
    mod filesystem {
        use super::*;
        use std::fs;
        use tempfile::TempDir;

        struct Fixture {
            root: TempDir,
            library: SkillLibrary,
        }

        impl Fixture {
            fn new() -> Self {
                let root = tempfile::tempdir().unwrap();
                let home = root.path().join("home");
                fs::create_dir_all(home.join(".cursor")).unwrap();
                let library = SkillLibrary::with_home(root.path().join("library"), home, |_| None);
                Self { root, library }
            }

            fn home(&self) -> PathBuf {
                self.root.path().join("home")
            }

            fn skill(&self, relative: &str, name: &str, body: &str) -> PathBuf {
                let dir = self.root.path().join(relative);
                fs::create_dir_all(&dir).unwrap();
                fs::write(
                    dir.join("SKILL.md"),
                    format!("---\nname: {name}\ndescription: About {name}\n---\n{body}\n"),
                )
                .unwrap();
                dir
            }

            fn apply(&self, op: SkillLibraryOp) -> Result<SkillLibraryView> {
                self.apply_in(op, None)
            }

            fn apply_in(
                &self,
                op: SkillLibraryOp,
                project: Option<&Path>,
            ) -> Result<SkillLibraryView> {
                self.library.apply_local(op, project)?;
                self.library.snapshot(project)
            }

            fn install(&self, dir: &Path) -> SkillLibraryView {
                self.apply(SkillLibraryOp::InstallPath { path: display(dir) })
                    .unwrap()
            }

            fn deploy(&self, skill: &str, harness: Option<HarnessId>) -> Result<SkillLibraryView> {
                self.apply(SkillLibraryOp::Deploy {
                    skill: skill.into(),
                    harness,
                })
            }
        }

        fn states(
            view: &SkillLibraryView,
            id: &str,
        ) -> Vec<(Option<HarnessId>, SkillDeploymentState)> {
            view.skills
                .iter()
                .find(|skill| skill.id == id)
                .unwrap()
                .deployments
                .iter()
                .map(|deployment| (deployment.harness, deployment.state))
                .collect()
        }

        #[test]
        fn installs_deploys_and_undeploys_a_local_skill() {
            let fixture = Fixture::new();
            let source = fixture.skill("sources/review", "review", "v1");
            let view = fixture.install(&source);
            assert_eq!(view.skills.len(), 1);
            assert_eq!(view.skills[0].id, "review");
            assert_eq!(view.skills[0].description, "About review");
            assert!(view.skills[0].deployments.is_empty());
            let cursor = view
                .targets
                .iter()
                .find(|target| target.harness == Some(HarnessId::Cursor))
                .unwrap();
            assert!(cursor.available);
            let claude = view
                .targets
                .iter()
                .find(|target| target.harness == Some(HarnessId::ClaudeCode))
                .unwrap();
            assert!(!claude.available);
            assert!(view.targets[0].harness.is_none() && view.targets[0].available);

            let view = fixture.deploy("review", Some(HarnessId::Cursor)).unwrap();
            let link = fixture.home().join(".cursor/skills/review");
            assert_eq!(
                fs::read_link(&link).unwrap(),
                fixture.root.path().join("library/skills/review")
            );
            assert_eq!(
                states(&view, "review"),
                [(Some(HarnessId::Cursor), SkillDeploymentState::Deployed)]
            );
            assert!(view.unmanaged.is_empty());

            let view = fixture
                .apply(SkillLibraryOp::Undeploy {
                    skill: "review".into(),
                    harness: Some(HarnessId::Cursor),
                })
                .unwrap();
            assert!(fs::symlink_metadata(&link).is_err());
            assert!(states(&view, "review").is_empty());
            assert!(source.join("SKILL.md").is_file());
        }

        #[test]
        fn an_existing_skill_is_never_overwritten_until_adopted() {
            let fixture = Fixture::new();
            fixture.install(&fixture.skill("sources/review", "review", "library"));
            let theirs = fixture.skill("home/.cursor/skills/review", "review", "mine");
            fs::write(theirs.join("notes.md"), "keep").unwrap();

            let error = fixture
                .deploy("review", Some(HarnessId::Cursor))
                .unwrap_err();
            assert!(error.to_string().contains("not managed"), "{error}");
            assert_eq!(fs::read_to_string(theirs.join("notes.md")).unwrap(), "keep");
            let view = fixture.library.snapshot(None).unwrap();
            assert_eq!(view.unmanaged.len(), 1);
            assert_eq!(view.unmanaged[0].harness, Some(HarnessId::Cursor));
            assert_eq!(view.unmanaged[0].name, "review");

            let view = fixture
                .apply(SkillLibraryOp::Adopt {
                    path: display(&theirs),
                })
                .unwrap();
            assert!(view.unmanaged.is_empty());
            let ids: Vec<&str> = view.skills.iter().map(|skill| skill.id.as_str()).collect();
            assert_eq!(ids, ["review", "review-2"]);
            assert_eq!(
                states(&view, "review-2"),
                [(Some(HarnessId::Cursor), SkillDeploymentState::Deployed)]
            );
            let stored = fixture.root.path().join("library/skills/review-2");
            assert_eq!(fs::read_to_string(stored.join("notes.md")).unwrap(), "keep");
            assert!(fs::symlink_metadata(&theirs).is_err());
            assert_eq!(
                fs::read_link(fixture.home().join(".cursor/skills/review-2")).unwrap(),
                stored
            );
        }

        #[test]
        fn adopting_refuses_paths_outside_known_skill_directories() {
            let fixture = Fixture::new();
            let stray = fixture.skill("elsewhere/review", "review", "x");
            let error = fixture
                .apply(SkillLibraryOp::Adopt {
                    path: display(&stray),
                })
                .unwrap_err();
            assert!(
                error.to_string().contains("known skills directory"),
                "{error}"
            );
            assert!(stray.join("SKILL.md").is_file());
        }

        #[test]
        fn reinstalling_from_the_same_folder_updates_in_place_and_refreshes_copies() {
            let fixture = Fixture::new();
            let source = fixture.skill("sources/review", "review", "v1");
            fixture.install(&source);
            fixture
                .apply(SkillLibraryOp::SetMode {
                    mode: SkillSyncMode::Copy,
                })
                .unwrap();
            let view = fixture.deploy("review", None).unwrap();
            assert_eq!(view.mode, SkillSyncMode::Copy);
            assert_eq!(view.skills[0].deployments[0].mode, SkillSyncMode::Copy);
            let copy = fixture.home().join(".agents/skills/review/SKILL.md");
            assert!(fs::read_to_string(&copy).unwrap().contains("v1"));

            fixture.skill("sources/review", "review", "v2");
            let view = fixture.install(&source);
            assert_eq!(view.skills.len(), 1);
            assert!(fs::read_to_string(&copy).unwrap().contains("v2"));

            let other = fixture.skill("other/review", "review", "different");
            let view = fixture.install(&other);
            let ids: Vec<&str> = view.skills.iter().map(|skill| skill.id.as_str()).collect();
            assert_eq!(ids, ["review", "review-2"]);
        }

        #[test]
        fn a_folder_of_skills_installs_each_and_presets_apply_all_or_nothing() {
            let fixture = Fixture::new();
            fixture.skill("pack/skills/alpha", "alpha", "a");
            fixture.skill("pack/skills/beta", "beta", "b");
            let view = fixture.install(&fixture.root.path().join("pack"));
            assert_eq!(view.skills.len(), 2);
            let view = fixture
                .apply(SkillLibraryOp::SavePreset {
                    id: None,
                    name: " Writing ".into(),
                    skills: vec![
                        "alpha".into(),
                        "missing".into(),
                        "beta".into(),
                        "alpha".into(),
                    ],
                })
                .unwrap();
            assert_eq!(view.presets[0].name, "Writing");
            assert_eq!(view.presets[0].skills, ["alpha", "beta"]);
            let preset = view.presets[0].id.clone();

            fixture.skill("home/.cursor/skills/beta", "beta", "theirs");
            let apply = |enabled| SkillLibraryOp::ApplyPreset {
                id: preset.clone(),
                harness: Some(HarnessId::Cursor),
                enabled,
            };
            assert!(fixture.apply(apply(true)).is_err());
            assert!(
                fs::symlink_metadata(fixture.home().join(".cursor/skills/alpha")).is_err(),
                "a refused preset deploys nothing"
            );

            fs::remove_dir_all(fixture.home().join(".cursor/skills/beta")).unwrap();
            let view = fixture.apply(apply(true)).unwrap();
            assert_eq!(states(&view, "alpha").len(), 1);
            assert_eq!(states(&view, "beta").len(), 1);
            let view = fixture.apply(apply(false)).unwrap();
            assert!(states(&view, "alpha").is_empty());
            assert!(states(&view, "beta").is_empty());

            let view = fixture
                .apply(SkillLibraryOp::Remove {
                    skill: "alpha".into(),
                })
                .unwrap();
            assert_eq!(view.presets[0].skills, ["beta"]);
            let view = fixture
                .apply(SkillLibraryOp::DeletePreset { id: preset })
                .unwrap();
            assert!(view.presets.is_empty());
        }

        #[test]
        fn removing_a_skill_clears_its_links_but_keeps_replaced_targets() {
            let fixture = Fixture::new();
            fixture.install(&fixture.skill("sources/review", "review", "v1"));
            fixture.deploy("review", None).unwrap();
            fixture.deploy("review", Some(HarnessId::Cursor)).unwrap();
            let replaced = fixture.home().join(".cursor/skills/review");
            fs::remove_file(&replaced).unwrap();
            fixture.skill("home/.cursor/skills/review", "review", "mine");

            let view = fixture.library.snapshot(None).unwrap();
            assert!(
                states(&view, "review")
                    .contains(&(Some(HarnessId::Cursor), SkillDeploymentState::Conflict))
            );

            let view = fixture
                .apply(SkillLibraryOp::Remove {
                    skill: "review".into(),
                })
                .unwrap();
            assert!(view.skills.is_empty());
            assert!(fs::symlink_metadata(fixture.home().join(".agents/skills/review")).is_err());
            assert!(replaced.join("SKILL.md").is_file());
            assert!(!fixture.root.path().join("library/skills/review").exists());
        }

        #[test]
        fn project_scope_deploys_into_the_project_folder() {
            let fixture = Fixture::new();
            fixture.install(&fixture.skill("sources/review", "review", "v1"));
            let project = fixture.root.path().join("project");
            fs::create_dir_all(&project).unwrap();
            let view = fixture
                .apply_in(
                    SkillLibraryOp::Deploy {
                        skill: "review".into(),
                        harness: Some(HarnessId::ClaudeCode),
                    },
                    Some(&project),
                )
                .unwrap();
            assert!(project.join(".claude/skills/review/SKILL.md").is_file());
            assert!(view.targets.iter().all(|target| target.available));
            assert_eq!(
                states(&view, "review"),
                [(Some(HarnessId::ClaudeCode), SkillDeploymentState::Deployed)]
            );
            assert!(states(&fixture.library.snapshot(None).unwrap(), "review").is_empty());
            let error = fixture
                .apply_in(
                    SkillLibraryOp::Deploy {
                        skill: "review".into(),
                        harness: Some(HarnessId::Amp),
                    },
                    Some(&project),
                )
                .unwrap_err();
            assert!(error.to_string().contains("shared target"), "{error}");
        }

        #[test]
        fn archives_install_without_escaping_their_destination() {
            use std::io::Write;
            let fixture = Fixture::new();
            let archive = fixture.root.path().join("notes.skill");
            let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("SKILL.md", options).unwrap();
            writer
                .write_all(b"---\ndescription: Packed\n---\nBody\n")
                .unwrap();
            writer.start_file("scripts/run.sh", options).unwrap();
            writer.write_all(b"echo hi\n").unwrap();
            writer.finish().unwrap();

            let view = fixture.install(&archive);
            assert_eq!(view.skills[0].id, "notes");
            assert_eq!(view.skills[0].description, "Packed");
            assert!(matches!(view.skills[0].source, SkillSource::Archive { .. }));
            assert!(
                fixture
                    .root
                    .path()
                    .join("library/skills/notes/scripts/run.sh")
                    .is_file()
            );

            let hostile = fixture.root.path().join("hostile.zip");
            let mut writer = zip::ZipWriter::new(fs::File::create(&hostile).unwrap());
            writer.start_file("../escaped/SKILL.md", options).unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
            assert!(
                fixture
                    .apply(SkillLibraryOp::InstallPath {
                        path: display(&hostile),
                    })
                    .is_err()
            );
            assert!(!fixture.root.path().join("escaped").exists());
            let unsupported = fixture.root.path().join("notes.txt");
            fs::write(&unsupported, "x").unwrap();
            assert!(
                fixture
                    .apply(SkillLibraryOp::InstallPath {
                        path: display(&unsupported),
                    })
                    .is_err()
            );
        }

        fn git(dir: &Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn git_sources_install_report_updates_and_update() {
            let fixture = Fixture::new();
            let repo = fixture.root.path().join("remote");
            fixture.skill("remote/skills/alpha", "alpha", "v1");
            fixture.skill("remote/skills/beta", "beta", "v1");
            git(&repo, &["init", "--quiet", "--initial-branch=main"]);
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "--quiet", "-m", "first"]);
            let url = format!("file://{}", repo.display());

            let view = fixture
                .library
                .apply(
                    SkillLibraryOp::InstallGit {
                        source: url.clone(),
                    },
                    None,
                )
                .await
                .unwrap();
            assert_eq!(view.skills.len(), 2);
            let SkillSource::Git {
                subpath, revision, ..
            } = &view.skills[0].source
            else {
                panic!("expected a git source");
            };
            assert_eq!(subpath.as_deref(), Some("skills/alpha"));
            assert!(revision.is_some());

            let view = fixture
                .library
                .apply(SkillLibraryOp::CheckUpdates, None)
                .await
                .unwrap();
            assert!(view.skills.iter().all(|skill| !skill.update_available));

            fixture.skill("remote/skills/alpha", "alpha", "v2");
            git(&repo, &["commit", "--quiet", "-am", "second"]);
            let view = fixture
                .library
                .apply(SkillLibraryOp::CheckUpdates, None)
                .await
                .unwrap();
            assert!(view.skills.iter().all(|skill| skill.update_available));

            let view = fixture
                .library
                .apply(
                    SkillLibraryOp::Update {
                        skill: "alpha".into(),
                    },
                    None,
                )
                .await
                .unwrap();
            assert!(!view.skills[0].update_available);
            assert!(view.skills[1].update_available);
            let stored = fixture.root.path().join("library/skills/alpha/SKILL.md");
            assert!(fs::read_to_string(stored).unwrap().contains("v2"));

            let error = fixture
                .library
                .apply(
                    SkillLibraryOp::InstallGit {
                        source: "ext::sh -c echo".into(),
                    },
                    None,
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("not a Git URL"), "{error}");
        }
    }
}
