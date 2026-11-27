//! Startup log rotation.
//!
//! Every process session writes to `<dir>/latest.log`. On the next start the previous
//! `latest.log` is first *staged* (renamed to `<stem>-YYYY-MM-DD_HH-MM-SS.log`, named after the
//! session start time recorded in its header), which is cheap and done synchronously so the new
//! session can immediately reuse `latest.log`. Staged files are then compressed into
//! `<stem>-YYYY-MM-DD_HH-MM-SS.tar.gz` and the oldest archives beyond the retention limit are
//! pruned. Compression writes to a `.partial` file first and renames atomically, so an interrupted
//! run never leaves a truncated archive behind; any staged `.log` left over is retried next start.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// File name of the log written by the currently running session.
pub const ACTIVE_LOG_NAME: &str = "latest.log";
/// Prefix of the first line of every session log; followed by an RFC 3339 timestamp.
pub const SESSION_HEADER_PREFIX: &str = "# l4a-log session_start=";

const TIMESTAMP_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";
const ARCHIVE_EXT: &str = ".tar.gz";
const PARTIAL_EXT: &str = ".tar.gz.partial";

/// Result of compressing staged logs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ArchiveReport {
    /// Newly written `.tar.gz` archives.
    pub archived: Vec<PathBuf>,
    /// Archives removed because they exceeded the retention limit.
    pub pruned: Vec<PathBuf>,
    /// Staged logs that could not be archived, with the error message.
    pub failed: Vec<(PathBuf, String)>,
}

/// Formats a timestamp the way archive and staged log names expect.
pub fn format_file_timestamp<Tz: TimeZone>(at: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.format(TIMESTAMP_FORMAT).to_string()
}

/// Builds the header line written at the top of a new session log.
pub fn session_header(started_at: &DateTime<Local>, details: &str) -> String {
    let details = details.replace(['\r', '\n'], " ");
    format!(
        "{SESSION_HEADER_PREFIX}{} {details}\n",
        started_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
    )
}

/// Reads the session start time from a log header, falling back to the file modification time.
fn previous_session_time(path: &Path) -> DateTime<Local> {
    if let Ok(file) = File::open(path) {
        let mut first = String::new();
        let mut reader = BufReader::new(file.take(512));
        if reader.read_line(&mut first).is_ok() {
            if let Some(rest) = first.strip_prefix(SESSION_HEADER_PREFIX) {
                let stamp = rest.split_whitespace().next().unwrap_or_default();
                if let Ok(parsed) = DateTime::parse_from_rfc3339(stamp) {
                    return parsed.with_timezone(&Local);
                }
            }
        }
    }
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(DateTime::<Local>::from)
        .unwrap_or_else(|_| Local::now())
}

/// Chooses `<stem>-<ts><suffix>` that collides with neither a staged log nor an archive.
fn unique_stem(dir: &Path, stem: &str, stamp: &str) -> String {
    let taken = |base: &str| {
        dir.join(format!("{base}.log")).exists()
            || dir.join(format!("{base}{ARCHIVE_EXT}")).exists()
    };
    let base = format!("{stem}-{stamp}");
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unused suffix always exists")
}

/// Renames the previous session's `latest.log` to a timestamped staged file.
///
/// Returns `Ok(None)` when there is no previous log (or it is empty, in which case it is removed).
pub fn stage_previous_log(dir: &Path, stem: &str) -> io::Result<Option<PathBuf>> {
    let active = dir.join(ACTIVE_LOG_NAME);
    let meta = match fs::metadata(&active) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if meta.len() == 0 {
        fs::remove_file(&active)?;
        return Ok(None);
    }
    let stamp = format_file_timestamp(&previous_session_time(&active));
    let staged = dir.join(format!("{}.log", unique_stem(dir, stem, &stamp)));
    fs::rename(&active, &staged)?;
    Ok(Some(staged))
}

/// Returns true for `<stem>-YYYY-MM-DD_HH-MM-SS[-N]<ext>` style file names.
fn matches_timestamped(name: &str, stem: &str, ext: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(ext))
    else {
        return false;
    };
    let stamp = rest.get(..19).unwrap_or_default();
    let suffix = rest.get(19..).unwrap_or_default();
    NaiveDateTime::parse_from_str(stamp, TIMESTAMP_FORMAT).is_ok()
        && (suffix.is_empty()
            || suffix
                .strip_prefix('-')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

fn list_matching(dir: &Path, stem: &str, ext: &str) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| matches_timestamped(name, stem, ext))
        {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Compresses a single log file into a sibling `.tar.gz` and removes the source on success.
pub fn archive_log_file(log_path: &Path) -> io::Result<PathBuf> {
    let file_name = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid log file name"))?;
    let base = file_name.strip_suffix(".log").unwrap_or(file_name);
    let dir = log_path.parent().unwrap_or_else(|| Path::new("."));
    let archive = dir.join(format!("{base}{ARCHIVE_EXT}"));
    let partial = dir.join(format!("{base}{PARTIAL_EXT}"));

    let result = (|| -> io::Result<()> {
        let out = File::create(&partial)?;
        let encoder = GzEncoder::new(io::BufWriter::new(out), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder.mode(tar::HeaderMode::Complete);
        let mut source = File::open(log_path)?;
        builder.append_file(file_name, &mut source)?;
        let encoder = builder.into_inner()?;
        let mut writer = encoder.finish()?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    fs::rename(&partial, &archive)?;
    if let Err(err) = fs::remove_file(log_path) {
        // Keep the source rather than risk a second archive with identical contents later.
        let _ = fs::remove_file(&archive);
        return Err(err);
    }
    Ok(archive)
}

fn modified_or_epoch(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Removes the oldest archives so that at most `keep` remain. `keep == 0` disables pruning.
pub fn prune_archives(dir: &Path, stem: &str, keep: usize) -> io::Result<Vec<PathBuf>> {
    if keep == 0 {
        return Ok(Vec::new());
    }
    let mut archives = list_matching(dir, stem, ARCHIVE_EXT)?;
    if archives.len() <= keep {
        return Ok(Vec::new());
    }
    // File names embed the session start time; mtime breaks ties for suffixed duplicates.
    archives.sort_by(|a, b| {
        a.file_name()
            .cmp(&b.file_name())
            .then_with(|| modified_or_epoch(a).cmp(&modified_or_epoch(b)))
    });
    let excess = archives.len() - keep;
    let mut removed = Vec::with_capacity(excess);
    for path in archives.into_iter().take(excess) {
        fs::remove_file(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

/// Compresses every staged log (except `exclude`, the log currently being written) and prunes
/// old archives. Stale `.partial` files from interrupted runs are cleaned up first.
pub fn archive_staged_logs(
    dir: &Path,
    stem: &str,
    keep: usize,
    exclude: Option<&Path>,
) -> io::Result<ArchiveReport> {
    for stale in list_matching(dir, stem, PARTIAL_EXT)? {
        let _ = fs::remove_file(stale);
    }
    let mut report = ArchiveReport::default();
    for staged in list_matching(dir, stem, ".log")? {
        if exclude.is_some_and(|exclude| exclude == staged) {
            continue;
        }
        match archive_log_file(&staged) {
            Ok(archive) => report.archived.push(archive),
            Err(err) => report.failed.push((staged, err.to_string())),
        }
    }
    report.pruned = prune_archives(dir, stem, keep)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    fn read_archive(path: &Path) -> Vec<(String, String)> {
        let mut archive = tar::Archive::new(GzDecoder::new(File::open(path).unwrap()));
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().into_owned();
                let mut body = String::new();
                entry.read_to_string(&mut body).unwrap();
                (name, body)
            })
            .collect()
    }

    #[test]
    fn stages_previous_log_using_session_header_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let started = Local.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
        fs::write(
            dir.path().join(ACTIVE_LOG_NAME),
            format!("{}line\n", session_header(&started, "pid=1")),
        )
        .unwrap();

        let staged = stage_previous_log(dir.path(), "l4a").unwrap().unwrap();
        assert_eq!(
            staged.file_name().unwrap().to_str().unwrap(),
            "l4a-2026-03-04_05-06-07.log"
        );
        assert!(!dir.path().join(ACTIVE_LOG_NAME).exists());
    }

    #[test]
    fn empty_or_missing_previous_log_is_not_staged() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stage_previous_log(dir.path(), "l4a").unwrap().is_none());
        fs::write(dir.path().join(ACTIVE_LOG_NAME), "").unwrap();
        assert!(stage_previous_log(dir.path(), "l4a").unwrap().is_none());
        assert!(!dir.path().join(ACTIVE_LOG_NAME).exists());
    }

    #[test]
    fn colliding_timestamps_get_numeric_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        let started = Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let header = session_header(&started, "");
        for _ in 0..3 {
            fs::write(dir.path().join(ACTIVE_LOG_NAME), format!("{header}x\n")).unwrap();
            stage_previous_log(dir.path(), "l4a").unwrap();
        }
        let names: Vec<_> = list_matching(dir.path(), "l4a", ".log")
            .unwrap()
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "l4a-2026-01-01_00-00-00-2.log",
                "l4a-2026-01-01_00-00-00-3.log",
                "l4a-2026-01-01_00-00-00.log",
            ]
        );
    }

    #[test]
    fn archives_staged_logs_into_tar_gz_and_removes_sources() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("l4a-2026-02-03_04-05-06.log");
        fs::write(&staged, "hello\nworld\n").unwrap();
        fs::write(
            dir.path().join("l4a-2026-02-03_04-05-06.tar.gz.partial"),
            "junk",
        )
        .unwrap();
        fs::write(dir.path().join("unrelated.log"), "keep").unwrap();

        let report = archive_staged_logs(dir.path(), "l4a", 10, None).unwrap();
        assert!(report.failed.is_empty());
        assert_eq!(report.archived.len(), 1);
        let archive = &report.archived[0];
        assert!(archive.ends_with("l4a-2026-02-03_04-05-06.tar.gz"));
        assert!(!staged.exists());
        assert!(!dir
            .path()
            .join("l4a-2026-02-03_04-05-06.tar.gz.partial")
            .exists());
        assert!(dir.path().join("unrelated.log").exists());
        assert_eq!(
            read_archive(archive),
            [(
                "l4a-2026-02-03_04-05-06.log".to_string(),
                "hello\nworld\n".to_string()
            )]
        );
    }

    #[test]
    fn excluded_active_file_is_not_archived() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("l4a-2026-02-03_04-05-06-2.log");
        fs::write(&own, "running").unwrap();
        let report = archive_staged_logs(dir.path(), "l4a", 10, Some(&own)).unwrap();
        assert!(report.archived.is_empty());
        assert!(own.exists());
    }

    #[test]
    fn prunes_oldest_archives_beyond_retention() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=5 {
            fs::write(
                dir.path()
                    .join(format!("l4a-2026-01-0{day}_00-00-00.tar.gz")),
                "x",
            )
            .unwrap();
        }
        fs::write(dir.path().join("other-2026-01-01_00-00-00.tar.gz"), "x").unwrap();
        let removed = prune_archives(dir.path(), "l4a", 2).unwrap();
        assert_eq!(removed.len(), 3);
        let left: Vec<_> = list_matching(dir.path(), "l4a", ARCHIVE_EXT)
            .unwrap()
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            left,
            [
                "l4a-2026-01-04_00-00-00.tar.gz",
                "l4a-2026-01-05_00-00-00.tar.gz"
            ]
        );
        assert!(dir.path().join("other-2026-01-01_00-00-00.tar.gz").exists());
        assert!(prune_archives(dir.path(), "l4a", 0).unwrap().is_empty());
    }

    #[test]
    fn timestamp_pattern_rejects_lookalikes() {
        assert!(matches_timestamped(
            "l4a-2026-01-01_00-00-00.log",
            "l4a",
            ".log"
        ));
        assert!(matches_timestamped(
            "l4a-2026-01-01_00-00-00-12.log",
            "l4a",
            ".log"
        ));
        assert!(!matches_timestamped(
            "l4a-server-2026-01-01_00-00-00.log",
            "l4a",
            ".log"
        ));
        assert!(!matches_timestamped("l4a-2026-01-01.log", "l4a", ".log"));
        assert!(!matches_timestamped(
            "l4a-2026-01-01_00-00-00-x.log",
            "l4a",
            ".log"
        ));
        assert!(!matches_timestamped(ACTIVE_LOG_NAME, "l4a", ".log"));
    }
}
