#![cfg_attr(windows, windows_subsystem = "windows")]

mod paths;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "octodock",
    version,
    about = "Local desktop controller for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Open a Zeron conversation URL.
    #[arg(value_name = "URL")]
    open_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    Status,
    #[cfg(target_os = "linux")]
    Appshot,
    Mcp,
}

/// mimalloc, macOS only: libmalloc never returns the streaming churn's
/// high-water pages, so transient allocation became permanent RSS
/// (docs/memory-plan.md §1). Pinned to mimalloc v2 in the workspace manifest —
/// the crate's default v3 has the same pathology (churn retained as permanent
/// RSS, ~6x glibc's growth on identical workloads, no idle recovery). Linux
/// measured flat on glibc, so it keeps the system allocator.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// glibc gives threads their own arenas (up to 8 × cores) and on free returns
/// only the top of each heap, so churn freed mid-heap stays resident: a
/// headless engine held 6.6GB in 129 arenas after a day. `malloc_trim` also
/// releases the free pages inside every arena, so long-running modes call it
/// once a minute. Capping arenas instead (`M_ARENA_MAX=2`) reclaimed less and
/// cost ~6x the engine's CPU in arena-lock contention while chats streamed
/// (docs/memory-plan.md).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn spawn_malloc_trimmer() {
    const PERIOD: std::time::Duration = std::time::Duration::from_secs(60);
    let spawned = std::thread::Builder::new()
        .name("malloc-trim".into())
        .spawn(|| {
            loop {
                std::thread::sleep(PERIOD);
                let started = std::time::Instant::now();
                // SAFETY: malloc_trim takes no pointers and locks each arena
                // itself, so it is safe to call from any thread at any time.
                let released = unsafe { libc::malloc_trim(0) } != 0;
                tracing::debug!(released, elapsed = ?started.elapsed(), "malloc_trim");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "malloc trimmer not started");
    }
}

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noop-browser")) {
        return Ok(());
    }
    #[cfg(windows)]
    attach_parent_console();
    let cli = Cli::parse();
    // Long-running modes log at info, one-shot CLI commands at warn (RUST_LOG
    // overrides either).
    // loro's internal block-encode diagnostics log at info and flood
    // journald on every snapshot export — enough to fill a disk on a
    // long-running headless host. Quiet them by default (RUST_LOG still
    // overrides the whole filter).
    let long_running = cli.command.is_none();
    let default_filter = if long_running {
        "info,loro_internal=warn,loro=warn"
    } else {
        "warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());
    // Long-running modes mirror stdout logging to {data_dir}/logs — a headed
    // app launched from Finder has no visible stdout, which left every sync
    // wedge report ("stale until restart") with zero diagnostics even though
    // the engine logs the exact failure line. One file per launch, previous
    // launch kept as `.old`.
    let log_file = if long_running {
        open_log_file("headed")
    } else {
        None
    };
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        // `zeron mcp` owns stdout for the protocol: a single log line on it
        // would corrupt the JSON-RPC stream, so its diagnostics go to stderr.
        if matches!(&cli.command, Some(Command::Mcp)) {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(std::io::stderr),
                )
                .init();
        } else {
            let registry = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer());
            match log_file {
                Some(file) => registry
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_ansi(false)
                            .with_writer(std::sync::Arc::new(file)),
                    )
                    .init(),
                None => registry.init(),
            }
        }
    }

    if long_running {
        // Finder launches have no visible stderr. Mirror the panic location
        // and backtrace into the same rotating log as engine diagnostics.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(panic = %info,
                backtrace = %std::backtrace::Backtrace::force_capture(),
                "application panic");
            default_hook(info);
        }));
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        spawn_malloc_trimmer();
    }

    match cli.command {
        Some(Command::Status) => {
            println!("Mode: local only");
            println!("Data dir: {}", paths::data_dir().display());
            Ok(())
        }
        Some(Command::Mcp) => {
            // One bridge per agent session, and it only relays: stdio
            // requests out to the engine over IPC and replies back. Two
            // workers keep a burst of parallel tool calls (large replies to
            // encode) close to per-core speed without a worker per core in
            // every agent's bridge.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            runtime.block_on(zeron_mcp::run(zeron_mcp::McpConfig::from_env()))
        }
        #[cfg(target_os = "linux")]
        Some(Command::Appshot) => {
            zeron_ui::appshots::request_running_appshot(&engine_config_from_env().data_dir)
                .map_err(anyhow::Error::msg)
        }
        None => {
            zeron_ui::run_app(zeron_ui::UiConfig {
                data_dir: paths::data_dir(),
                ipc_port: std::env::var("ZERON_IPC_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(27655),
                edge_url: String::new(),
                workos_client_id: None,
                edge_token: None,
                org_id: None,
                default_harness: harness_from_env(),
                initial_url: cli.open_url,
            });
            Ok(())
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // The GUI subsystem prevents Explorer from creating a console at startup.
    // Reuse an existing parent's console for CLI output and cargo run, without
    // allocating one. Attach before Clap so help and argument errors work too.
    // Preserve redirected pipes/files: attaching may replace standard handles.
    unsafe {
        let saved = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .map(|id| (id, GetStdHandle(id)));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, handle) in saved {
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetStdHandle(id, handle);
                }
            }
        }
    }
}

fn engine_config_from_env() -> zeron_engine::EngineConfig {
    zeron_engine::EngineConfig {
        data_dir: paths::data_dir(),
        edge_url: String::new(),
        ipc_port: std::env::var("ZERON_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(27655),
        default_harness: harness_from_env(),
        org_id: None,
        workos_client_id: None,
        edge_token: None,
    }
}

/// `ZERON_HARNESS` (kebab-case id) picks the default harness for chats without a
/// config row — `mock` powers the e2e smoke; default `claude-code`.
fn harness_from_env() -> zeron_engine::HarnessId {
    match std::env::var("ZERON_HARNESS").as_deref().map(str::trim) {
        Ok("mock") => zeron_engine::HarnessId::Mock,
        Ok("codex") => zeron_engine::HarnessId::Codex,
        Ok("cursor") => zeron_engine::HarnessId::Cursor,
        Ok("devin") => zeron_engine::HarnessId::Devin,
        Ok("grok") => zeron_engine::HarnessId::Grok,
        Ok("hermes") => zeron_engine::HarnessId::Hermes,
        Ok("pi") => zeron_engine::HarnessId::Pi,
        Ok("antigravity") => zeron_engine::HarnessId::Antigravity,
        Ok("amp") => zeron_engine::HarnessId::Amp,
        _ => zeron_engine::HarnessId::ClaudeCode,
    }
}

/// `{data_dir}/logs/zeron-{mode}.log`, previous launch preserved as `.old`.
/// Headed and headless are separate files so an embedded-engine app and a
/// daemon on the same machine never interleave writes.
///
/// The returned file holds an exclusive `flock` for the process lifetime:
/// rotate-on-launch is only safe when nothing is still WRITING the current
/// file. On 2026-08-04 a dev build launched twice next to the running
/// installed app — the first rename put the daemon's live log at `.old`, the
/// second unlinked it entirely, and the daemon spent the rest of the incident
/// logging to an orphaned inode (an entire day of sync diagnostics gone at
/// the exact moment they were needed). A launch that finds the canonical file
/// locked logs to `zeron-{mode}.{pid}.log` instead; the next lock-holding
/// launch sweeps pid-suffixed files older than a week.
fn open_log_file(mode: &str) -> Option<std::fs::File> {
    let dir = paths::data_dir().join("logs");
    open_log_file_in(&dir, mode)
}

/// Dir-parameterized body of [`open_log_file`] (unit-testable without env).
fn open_log_file_in(dir: &std::path::Path, mode: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("zeron-{mode}.log"));
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Probe the CURRENT inode for a live writer before touching it.
        let preexisting = path.exists();
        let existing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()?;
        let rc = unsafe { libc::flock(existing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            // A live process owns the canonical log — leave it alone.
            return std::fs::File::create(
                dir.join(format!("zeron-{mode}.{}.log", std::process::id())),
            )
            .ok();
        }
        // No live writer: rotate, create fresh, and lock it as ours. (The
        // probe's flock dies with `existing`; a first-ever launch has nothing
        // to rotate — the probe itself created the empty file.)
        drop(existing);
        if preexisting {
            let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        }
        let file = std::fs::File::create(&path).ok()?;
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        sweep_stale_pid_logs(dir, mode);
        Some(file)
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::rename(&path, dir.join(format!("zeron-{mode}.log.old")));
        std::fs::File::create(&path).ok()
    }
}

#[cfg(all(test, unix))]
mod log_file_tests {
    use super::open_log_file_in;

    #[test]
    fn second_launch_never_rotates_a_live_processes_log() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // First launch owns the canonical file and keeps writing.
        let first = open_log_file_in(dir, "headed").expect("first log");
        assert!(dir.join("zeron-headed.log").is_file());
        // Second launch while the first is alive: canonical file untouched,
        // pid-suffixed overflow file instead (the 2026-08-04 clobber).
        let second = open_log_file_in(dir, "headed").expect("second log");
        let pid_path = dir.join(format!("zeron-headed.{}.log", std::process::id()));
        assert!(pid_path.is_file(), "expected pid-suffixed overflow log");
        assert!(
            !dir.join("zeron-headed.log.old").exists(),
            "live canonical log must not be rotated away"
        );
        drop(second);
        // After the owner exits, a fresh launch rotates normally.
        drop(first);
        let third = open_log_file_in(dir, "headed").expect("third log");
        assert!(
            dir.join("zeron-headed.log.old").is_file(),
            "rotation resumes"
        );
        drop(third);
    }
}

/// Delete `zeron-{mode}.{pid}.log` overflow files older than a week — they
/// only exist when a second instance raced a live one for the canonical log.
#[cfg(unix)]
fn sweep_stale_pid_logs(dir: &std::path::Path, mode: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("zeron-{mode}.");
    let week = std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if !middle.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
