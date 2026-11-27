//! Shared logging for the Left4Addons desktop engine and the `l4a-server` catalog service.
//!
//! * Every session writes timestamped (local date + time) records to `<dir>/latest.log` and,
//!   optionally, to the console.
//! * On startup the previous `latest.log` is rolled into `<stem>-YYYY-MM-DD_HH-MM-SS.tar.gz`
//!   (see [`rotate`]); compression and retention pruning run on a background thread.
//! * [`http`] and [`process`] provide audited wrappers for outbound network requests and
//!   external commands; [`redact`] masks credentials in URLs and command lines.
//! * Records emitted through the `log` crate (e.g. by Tauri) are bridged into the same sinks,
//!   and panics are written to the log before the default hook runs.
//!
//! The filter is read from `L4A_LOG` (falling back to `RUST_LOG`), e.g.
//! `L4A_LOG=info,l4a::net=debug`.

pub mod http;
pub mod process;
pub mod redact;
pub mod rotate;

use chrono::Local;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub use rotate::{ArchiveReport, ACTIVE_LOG_NAME};

/// Environment variable holding the log filter directives.
pub const LOG_ENV_VAR: &str = "L4A_LOG";
/// Default number of compressed session archives to keep.
pub const DEFAULT_KEEP_ARCHIVES: usize = 30;
/// Default filter used when neither `L4A_LOG` nor `RUST_LOG` is set.
pub const DEFAULT_DIRECTIVES: &str = "info";

/// Where console output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleTarget {
    None,
    Stdout,
    Stderr,
}

/// Options for [`init`].
#[derive(Debug, Clone)]
pub struct LogOptions {
    /// Directory for `latest.log` and the archives. Created if missing.
    pub dir: PathBuf,
    /// Archive name prefix, e.g. `l4a` or `l4a-server`.
    pub file_stem: String,
    /// Application name and version written into the session header.
    pub app_label: String,
    /// Filter directives used when no environment override is present.
    pub default_directives: String,
    /// Number of `.tar.gz` archives to keep (`0` = unlimited).
    pub keep_archives: usize,
    pub console: ConsoleTarget,
}

impl LogOptions {
    pub fn new(dir: impl Into<PathBuf>, file_stem: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            file_stem: file_stem.into(),
            app_label: String::new(),
            default_directives: DEFAULT_DIRECTIVES.into(),
            keep_archives: DEFAULT_KEEP_ARCHIVES,
            console: ConsoleTarget::Stderr,
        }
    }
}

/// Handle describing the initialized logging session.
#[derive(Debug)]
pub struct LogSession {
    /// Log directory.
    pub dir: PathBuf,
    /// File currently being written, if file logging could be enabled.
    pub active_log: Option<PathBuf>,
    /// Previous session log that was staged for compression on this start.
    pub staged_previous: Option<PathBuf>,
    /// Whether this call installed the global subscriber (false if one already existed).
    pub installed: bool,
    archiver: Option<JoinHandle<()>>,
}

impl LogSession {
    /// Blocks until background compression of previous logs has finished.
    pub fn wait_for_archiver(&mut self) {
        if let Some(handle) = self.archiver.take() {
            let _ = handle.join();
        }
    }
}

/// Local-time timestamps with date, e.g. `2026-11-26 20:55:12.123 -08:00`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        write!(w, "{}", Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z"))
    }
}

fn build_filter(default_directives: &str) -> EnvFilter {
    for var in [LOG_ENV_VAR, "RUST_LOG"] {
        if let Ok(value) = std::env::var(var) {
            if let Ok(filter) = EnvFilter::try_new(value.trim()) {
                if !value.trim().is_empty() {
                    return filter;
                }
            }
        }
    }
    EnvFilter::try_new(default_directives).unwrap_or_else(|_| EnvFilter::new(DEFAULT_DIRECTIVES))
}

/// Rotates the previous log and opens a fresh active log file.
///
/// Returns `(active_path, file, staged_previous)`.
fn open_session_file(
    opts: &LogOptions,
    started_at: &chrono::DateTime<Local>,
) -> io::Result<(PathBuf, File, Option<PathBuf>)> {
    fs::create_dir_all(&opts.dir)?;
    let staged = match rotate::stage_previous_log(&opts.dir, &opts.file_stem) {
        Ok(staged) => staged,
        Err(err) => {
            eprintln!(
                "[l4a-logging] failed to rotate previous log {}: {err}",
                opts.dir.join(ACTIVE_LOG_NAME).display()
            );
            None
        }
    };

    let active = opts.dir.join(ACTIVE_LOG_NAME);
    let (path, file) = if active.exists() {
        // Another instance still holds latest.log (rename refused); keep it intact and write to
        // a timestamped file instead. It is archived on a later start.
        let fallback = opts.dir.join(format!(
            "{}-{}-{}.log",
            opts.file_stem,
            rotate::format_file_timestamp(started_at),
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&fallback)?;
        (fallback, file)
    } else {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&active)?;
        (active, file)
    };

    let mut file = file;
    let details = format!(
        "app=\"{}\" pid={} os={}/{}",
        opts.app_label,
        std::process::id(),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    file.write_all(rotate::session_header(started_at, &details).as_bytes())?;
    Ok((path, file, staged))
}

fn spawn_archiver(opts: &LogOptions, exclude: Option<PathBuf>) -> Option<JoinHandle<()>> {
    let dir = opts.dir.clone();
    let stem = opts.file_stem.clone();
    let keep = opts.keep_archives;
    std::thread::Builder::new()
        .name("l4a-log-archiver".into())
        .spawn(move || {
            match rotate::archive_staged_logs(&dir, &stem, keep, exclude.as_deref()) {
                Ok(report) => {
                    for archive in &report.archived {
                        tracing::info!(archive = %archive.display(), "Archived previous log");
                    }
                    for pruned in &report.pruned {
                        tracing::info!(archive = %pruned.display(), "Deleted expired log archive per retention policy");
                    }
                    for (path, err) in &report.failed {
                        tracing::warn!(file = %path.display(), error = %err, "Failed to archive previous log; will retry on next startup");
                    }
                }
                Err(err) => tracing::warn!(dir = %dir.display(), error = %err, "Failed to scan log directory"),
            }
        })
        .ok()
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".into());
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_default();
        let thread = std::thread::current();
        tracing::error!(
            target: "l4a::panic",
            thread = thread.name().unwrap_or("<unnamed>"),
            location = %location,
            backtrace = %std::backtrace::Backtrace::capture(),
            "Thread panicked: {payload}"
        );
        previous(info);
    }));
}

/// Installs the global subscriber with console + dated file output and rotates old logs.
///
/// File logging problems never abort startup: they are reported on stderr and in the console
/// log, and the session continues with console output only.
pub fn init(opts: LogOptions) -> LogSession {
    let started_at = Local::now();
    let (file_info, file_error) = match open_session_file(&opts, &started_at) {
        Ok(info) => (Some(info), None),
        Err(err) => (None, Some(err)),
    };

    let mut active_log = None;
    let mut staged_previous = None;
    let file_layer = file_info.map(|(path, file, staged)| {
        active_log = Some(path);
        staged_previous = staged;
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_timer(LocalTimer)
            .with_target(true)
            .with_thread_names(true)
            .with_writer(Mutex::new(file))
    });
    let stdout_layer = (opts.console == ConsoleTarget::Stdout).then(|| {
        tracing_subscriber::fmt::layer()
            .with_timer(LocalTimer)
            .with_writer(io::stdout)
    });
    let stderr_layer = (opts.console == ConsoleTarget::Stderr).then(|| {
        tracing_subscriber::fmt::layer()
            .with_timer(LocalTimer)
            .with_writer(io::stderr)
    });

    let installed = tracing_subscriber::registry()
        .with(build_filter(&opts.default_directives))
        .with(file_layer)
        .with(stdout_layer)
        .with(stderr_layer)
        .try_init()
        .is_ok();
    if installed {
        install_panic_hook();
    }

    if let Some(err) = &file_error {
        eprintln!(
            "[l4a-logging] cannot write to log directory {}: {err}",
            opts.dir.display()
        );
        tracing::warn!(dir = %opts.dir.display(), error = %err, "File logging unavailable; logging to console only");
    }
    tracing::info!(
        app = %opts.app_label,
        pid = std::process::id(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        log_file = %active_log.as_deref().map(Path::display).map(|d| d.to_string()).unwrap_or_default(),
        previous_log = %staged_previous.as_deref().map(Path::display).map(|d| d.to_string()).unwrap_or_default(),
        "Logging initialized"
    );

    let archiver = if file_error.is_none() {
        spawn_archiver(&opts, active_log.clone())
    } else {
        None
    };

    LogSession {
        dir: opts.dir,
        active_log,
        staged_previous,
        installed,
        archiver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_rotates_previous_log_and_writes_dated_records() {
        let dir = tempfile::tempdir().unwrap();
        let started = Local::now() - chrono::Duration::hours(1);
        fs::write(
            dir.path().join(ACTIVE_LOG_NAME),
            format!("{}old session\n", rotate::session_header(&started, "")),
        )
        .unwrap();

        let mut opts = LogOptions::new(dir.path(), "l4a-test");
        opts.app_label = "test 1.0".into();
        opts.console = ConsoleTarget::None;
        let mut session = init(opts);
        session.wait_for_archiver();
        assert!(session.installed);
        tracing::info!(target: "l4a::test", answer = 42, "hello log");

        let expected_archive = dir.path().join(format!(
            "l4a-test-{}.tar.gz",
            rotate::format_file_timestamp(&started)
        ));
        assert!(expected_archive.exists(), "missing {expected_archive:?}");
        assert_eq!(
            session.active_log.as_deref(),
            Some(dir.path().join(ACTIVE_LOG_NAME).as_path())
        );

        let text = fs::read_to_string(dir.path().join(ACTIVE_LOG_NAME)).unwrap();
        let mut lines = text.lines();
        assert!(lines
            .next()
            .unwrap()
            .starts_with(rotate::SESSION_HEADER_PREFIX));
        let record = text.lines().find(|l| l.contains("hello log")).unwrap();
        let today = Local::now().format("%Y-%m-%d").to_string();
        assert!(record.starts_with(&today), "record lacks date: {record}");
        assert!(record.contains("l4a::test"));
        assert!(record.contains("answer=42"));
        assert!(
            !record.contains('\u{1b}'),
            "file log must not contain ANSI codes"
        );
    }
}
