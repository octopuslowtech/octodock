use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::{App, Task};

use crate::icons;
use crate::popover;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTargetLaunch {
    Command(PathBuf),
    FileManager,
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub tint: Option<u32>,
    pub launch: OpenTargetLaunch,
}

#[derive(Default)]
pub struct OpenTargetsController {
    pub cwd: Option<PathBuf>,
    pub targets: Vec<OpenTarget>,
    pub menu: popover::Popup<()>,
    pub task: Option<Task<()>>,
}

struct Editor {
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    tint: Option<u32>,
    commands: &'static [&'static str],
}

const fn editor(
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    tint: Option<u32>,
    commands: &'static [&'static str],
) -> Editor {
    Editor {
        id,
        label,
        icon,
        tint,
        commands,
    }
}

const VSCODE_BLUE: Option<u32> = Some(0x23A9F2);

const EDITORS: &[Editor] = &[
    editor("cursor", "Cursor", icons::CURSOR_MARK, None, &["cursor"]),
    editor("vscode", "VS Code", icons::VSCODE_MARK, VSCODE_BLUE, &["code"]),
    editor(
        "vscode-insiders",
        "VS Code Insiders",
        icons::VSCODE_MARK,
        None,
        &["code-insiders"],
    ),
    editor("vscodium", "VSCodium", icons::VSCODE_MARK, None, &["codium"]),
    editor("zed", "Zed", icons::ZED_MARK, None, &["zed", "zeditor"]),
    editor(
        "android-studio",
        "Android Studio",
        icons::TERMINAL,
        None,
        &["studio", "android-studio"],
    ),
    editor(
        "intellij-idea",
        "IntelliJ IDEA",
        icons::TERMINAL,
        None,
        &["idea", "intellij-idea-ultimate", "intellij-idea-community"],
    ),
    editor("clion", "CLion", icons::TERMINAL, None, &["clion"]),
    editor("goland", "GoLand", icons::TERMINAL, None, &["goland"]),
    editor("phpstorm", "PhpStorm", icons::TERMINAL, None, &["phpstorm"]),
    editor(
        "pycharm",
        "PyCharm",
        icons::TERMINAL,
        None,
        &["pycharm", "pycharm-professional", "pycharm-community"],
    ),
    editor("rider", "Rider", icons::TERMINAL, None, &["rider", "rider64"]),
    editor("rubymine", "RubyMine", icons::TERMINAL, None, &["rubymine"]),
    editor("rustrover", "RustRover", icons::TERMINAL, None, &["rustrover"]),
    editor("webstorm", "WebStorm", icons::TERMINAL, None, &["webstorm"]),
];

const FILE_MANAGER_LABEL: &str = if cfg!(target_os = "macos") {
    "Finder"
} else if cfg!(target_os = "windows") {
    "Explorer"
} else {
    "Files"
};

const EXECUTABLE_EXTENSIONS: &[&str] = if cfg!(target_os = "windows") {
    &[".exe", ".cmd", ".bat"]
} else {
    &[""]
};

fn resolve_command(commands: &[&str], path_var: Option<&OsStr>) -> Option<PathBuf> {
    let directories: Vec<PathBuf> = std::env::split_paths(path_var?).collect();
    commands.iter().find_map(|command| {
        directories.iter().find_map(|directory| {
            EXECUTABLE_EXTENSIONS
                .iter()
                .map(|extension| directory.join(format!("{command}{extension}")))
                .find(|candidate| candidate.is_file())
        })
    })
}

pub fn github_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let rest = remote.split_once("://").map_or(remote, |(_, rest)| rest);
    let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
    let (host, path) = rest.split_once([':', '/'])?;
    let host = host.to_ascii_lowercase();
    let github = host == "github.com" || (!host.contains('.') && host.contains("github"));
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let valid = github
        && parts.next().is_some_and(|owner| !owner.is_empty())
        && parts.next().is_some_and(|repo| !repo.is_empty())
        && parts.next().is_none();
    valid.then(|| format!("https://github.com/{path}"))
}

fn origin_remote(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn detect(cwd: &Path) -> Vec<OpenTarget> {
    let path_var = std::env::var_os("PATH");
    let mut targets: Vec<OpenTarget> = EDITORS
        .iter()
        .filter_map(|editor| {
            let command = resolve_command(editor.commands, path_var.as_deref())?;
            Some(OpenTarget {
                id: editor.id,
                label: editor.label,
                icon: editor.icon,
                tint: editor.tint,
                launch: OpenTargetLaunch::Command(command),
            })
        })
        .collect();
    targets.push(OpenTarget {
        id: "file-manager",
        label: FILE_MANAGER_LABEL,
        icon: icons::FOLDER,
        tint: None,
        launch: OpenTargetLaunch::FileManager,
    });
    if let Some(url) = origin_remote(cwd).as_deref().and_then(github_url) {
        targets.push(OpenTarget {
            id: "github",
            label: "GitHub",
            icon: icons::GITHUB_MARK,
            tint: None,
            launch: OpenTargetLaunch::Url(url),
        });
    }
    targets
}

pub fn preferred<'a>(targets: &'a [OpenTarget], id: Option<&str>) -> Option<&'a OpenTarget> {
    id.and_then(|id| targets.iter().find(|target| target.id == id))
        .or_else(|| targets.first())
}

pub fn open(target: &OpenTarget, cwd: &Path, cx: &App) -> std::io::Result<()> {
    match &target.launch {
        OpenTargetLaunch::Command(command) => {
            let mut child = Command::new(command)
                .arg(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        OpenTargetLaunch::FileManager => cx.open_with_system(cwd),
        OpenTargetLaunch::Url(url) => cx.open_url(url),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &'static str) -> OpenTarget {
        OpenTarget {
            id,
            label: id,
            icon: icons::FOLDER,
            tint: None,
            launch: OpenTargetLaunch::FileManager,
        }
    }

    #[test]
    fn github_urls_come_from_https_ssh_and_alias_remotes() {
        let expected = Some("https://github.com/owner/repo".to_string());
        for remote in [
            "https://github.com/owner/repo.git\n",
            "https://github.com/owner/repo",
            "https://token@github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
            "git@github-work:owner/repo.git",
        ] {
            assert_eq!(github_url(remote), expected, "{remote}");
        }
        for remote in [
            "git@gitlab.com:owner/repo.git",
            "https://github.example.com/owner/repo.git",
            "/srv/git/repo.git",
            "../repo",
            "https://github.com/owner",
            "https://github.com/owner/repo/extra",
            "",
        ] {
            assert_eq!(github_url(remote), None, "{remote}");
        }
    }

    #[test]
    fn preferred_falls_back_to_the_first_target() {
        let targets = [target("vscode"), target("rider")];
        assert_eq!(preferred(&targets, Some("rider")).unwrap().id, "rider");
        assert_eq!(preferred(&targets, Some("gone")).unwrap().id, "vscode");
        assert_eq!(preferred(&targets, None).unwrap().id, "vscode");
        assert!(preferred(&[], Some("rider")).is_none());
    }

    #[test]
    fn commands_resolve_from_path_in_candidate_order() {
        let directory =
            std::env::temp_dir().join(format!("zeron-open-targets-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(format!("rider64{}", EXECUTABLE_EXTENSIONS[0]));
        std::fs::write(&executable, "").unwrap();
        let path_var = std::env::join_paths([directory.join("missing"), directory.clone()]).unwrap();

        assert_eq!(
            resolve_command(&["rider", "rider64"], Some(&path_var)),
            Some(executable)
        );
        assert_eq!(resolve_command(&["rider"], Some(&path_var)), None);
        assert_eq!(resolve_command(&["rider64"], None), None);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
