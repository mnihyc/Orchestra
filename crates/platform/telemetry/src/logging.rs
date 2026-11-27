//! Structured component and request context with sensitive data excluded, written where the component runs and sent nowhere.
//!
//! Lines go to standard error, as text for a person or as JSON for a
//! collector. What is logged is events and identifiers: never a secret, a
//! token, a credential or what was said in a conversation. The level is
//! `info` unless `ORCH_LOG` names another filter.
//!
//! A line that cannot be written (a full disk, a standard error nobody reads) is lost, and only that line: the
//! next one begins on a line of its own, and writing never fails the code that logs. That code is any task of the
//! process, the link's reconnecting among them, so a log that could not be written must never end it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::{
    EnvFilter, Layer as _, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

/// How a line is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// For a person reading a terminal.
    Text,
    /// One JSON object a line.
    Json,
}

impl LogFormat {
    /// The format named `name` in a configuration file; text for anything else.
    #[must_use]
    pub fn named(name: &str) -> Self {
        if name == "json" {
            Self::Json
        } else {
            Self::Text
        }
    }
}

/// The variable that names a filter other than `info`, as `debug` or
/// `orch_link=trace,info`.
pub const FILTER_VARIABLE: &str = "ORCH_LOG";
/// Set by the connector's launchd service: structured logs go only to rotating daily files.
pub const FILES_ONLY_VARIABLE: &str = "ORCH_CONNECTOR_LOG_FILES_ONLY";

/// Starts logging for `component`. A second call in one process changes
/// nothing.
pub fn init(component: &'static str, format: LogFormat) {
    let filter =
        EnvFilter::try_from_env(FILTER_VARIABLE).unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(crate::database::layer());
    let started = match format {
        LogFormat::Json => registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(StandardError::default())
                    .with_filter(filter),
            )
            .try_init(),
        LogFormat::Text => registry
            .with(
                fmt::layer()
                    .with_writer(StandardError::default())
                    .with_filter(filter),
            )
            .try_init(),
    };
    if started.is_ok() {
        tracing::info!(component, "logging started");
    }
}

/// Starts logging for `component` to standard error and to a file a day in `dir`
/// (`<component>-YYYY-MM-DD.log`, by the UTC date), where the files of the last `keep_days`
/// days are kept and older ones removed as a day begins: a machine's own log, for whoever
/// debugs that machine, which is never sent anywhere. A directory that cannot be written leaves
/// standard error alone. A second call in one process changes nothing.
pub fn init_with_files(component: &'static str, format: LogFormat, dir: &Path, keep_days: u64) {
    init_files(component, format, dir, keep_days, true);
}

/// Starts daily-file logging without a standard-error tracing mirror; Rust's panic hook stays intact.
pub fn init_files_only(component: &'static str, format: LogFormat, dir: &Path, keep_days: u64) {
    init_files(component, format, dir, keep_days, false);
}

fn init_files(
    component: &'static str,
    format: LogFormat,
    dir: &Path,
    keep_days: u64,
    mirror: bool,
) {
    let filter =
        EnvFilter::try_from_env(FILTER_VARIABLE).unwrap_or_else(|_| EnvFilter::new("info"));
    let files = DailyFiles {
        dir: dir.to_owned(),
        component,
        keep_days,
        open: Mutex::new(None),
    };
    let registry = tracing_subscriber::registry().with(crate::database::layer());
    // The formatter's own report of a write that failed is an `eprintln!`, which panics where standard error cannot
    // be written either (one full disk under both): the panic ended whichever task had logged. The writers here
    // never fail, and the report stays off as well.
    let started = match format {
        LogFormat::Json => registry
            .with(mirror.then(|| {
                fmt::layer()
                    .json()
                    .with_writer(StandardError::default())
                    .with_filter(filter.clone())
            }))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(files)
                    .log_internal_errors(false)
                    .with_filter(filter),
            )
            .try_init(),
        LogFormat::Text => registry
            .with(mirror.then(|| {
                fmt::layer()
                    .with_writer(StandardError::default())
                    .with_filter(filter.clone())
            }))
            .with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(files)
                    .log_internal_errors(false)
                    .with_filter(filter),
            )
            .try_init(),
    };
    if started.is_ok() {
        tracing::info!(component, dir = %dir.display(), keep_days, "logging started");
    }
}

/// The seconds of a day.
const DAY_SECS: u64 = 86_400;

/// The UTC date `days` days after 1970-01-01, as `YYYY-MM-DD` (the proleptic Gregorian calendar).
fn date_of(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_shifted + 2) / 5 + 1;
    let month = if month_shifted < 10 {
        month_shifted + 3
    } else {
        month_shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The days since 1970-01-01 now.
fn today() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    i64::try_from(secs / DAY_SECS).unwrap_or(0)
}

/// Writes `line` to `sink`, so that a line that fails is the only one lost: where only the beginning of the last
/// line got there (a full disk takes what still fits), `cut` says so, and that part is ended before this line
/// begins. The error is the caller's to drop.
fn write_line(sink: &mut impl std::io::Write, cut: &mut bool, line: &[u8]) -> std::io::Result<()> {
    if *cut {
        sink.write_all(b"\n")?;
        *cut = false;
    }
    let mut rest = line;
    while !rest.is_empty() {
        match sink.write(rest) {
            Ok(0) => {
                *cut = rest.len() < line.len();
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            Ok(written) => rest = rest.get(written..).unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                *cut = rest.len() < line.len();
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Standard error, a whole line at a time, as the files are written.
#[derive(Default)]
struct StandardError {
    cut: Mutex<bool>,
}

/// One line to standard error.
struct StandardErrorLine<'a> {
    cut: &'a Mutex<bool>,
}

impl std::io::Write for StandardErrorLine<'_> {
    fn write(&mut self, line: &[u8]) -> std::io::Result<usize> {
        self.write_all(line)?;
        Ok(line.len())
    }

    /// The formatter hands over each event as one whole line. One that cannot be written is lost; this never fails.
    fn write_all(&mut self, line: &[u8]) -> std::io::Result<()> {
        let mut cut = self
            .cut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = write_line(&mut std::io::stderr().lock(), &mut cut, line);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        Ok(())
    }
}

impl<'a> fmt::MakeWriter<'a> for StandardError {
    type Writer = StandardErrorLine<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        StandardErrorLine { cut: &self.cut }
    }
}

/// The file of a day, open, and whether its last line was cut.
struct DayFile {
    date: String,
    file: std::fs::File,
    cut: bool,
}

/// The files a component logs to, one a day.
struct DailyFiles {
    dir: PathBuf,
    component: &'static str,
    keep_days: u64,
    open: Mutex<Option<DayFile>>,
}

impl DailyFiles {
    /// The file of `date`, made with its directory where they are not.
    fn file_of(&self, date: &str) -> std::io::Result<std::fs::File> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(format!("{}-{date}.log", self.component)))
    }

    /// Removes the files of this component older than the days kept, `today` the date now.
    fn prune(&self, today: i64) {
        let keep = i64::try_from(self.keep_days).unwrap_or(i64::MAX);
        let oldest = date_of(today.saturating_sub(keep));
        let prefix = format!("{}-", self.component);
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(date) = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".log"))
            else {
                continue;
            };
            // Dates written as YYYY-MM-DD compare as text.
            if date.len() == oldest.len() && date < oldest.as_str() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// A write into the file of the day it is made on.
struct DailyWriter<'a> {
    files: &'a DailyFiles,
}

impl std::io::Write for DailyWriter<'_> {
    fn write(&mut self, line: &[u8]) -> std::io::Result<usize> {
        self.write_all(line)?;
        Ok(line.len())
    }

    /// The formatter hands over each event as one whole line. One that cannot be written is lost; this never fails.
    fn write_all(&mut self, line: &[u8]) -> std::io::Result<()> {
        let mut open = self
            .files
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let day = today();
        let date = date_of(day);
        if open.as_ref().is_none_or(|file| file.date != date) {
            // A file that cannot be opened drops the line here and is retried at the next one.
            // The optional standard-error mirror is independent; files-only services keep it off.
            *open = self.files.file_of(&date).ok().map(|file| DayFile {
                date,
                file,
                cut: false,
            });
            self.files.prune(day);
        }
        if let Some(open) = open.as_mut() {
            let _ = write_line(&mut open.file, &mut open.cut, line);
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut open = self
            .files
            .open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(open) = open.as_mut() {
            let _ = open.file.flush();
        }
        Ok(())
    }
}

impl<'a> fmt::MakeWriter<'a> for DailyFiles {
    type Writer = DailyWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        DailyWriter { files: self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt::MakeWriter as _;

    #[test]
    fn a_day_number_is_its_calendar_date() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(59), "1970-03-01");
        assert_eq!(date_of(11_016), "2000-02-29");
        assert_eq!(date_of(20_740), "2026-10-14");
        assert_eq!(date_of(-1), "1969-12-31");
    }

    #[test]
    fn a_line_goes_to_the_file_of_its_day_and_files_older_than_kept_go() {
        let dir = tempfile::tempdir().unwrap();
        let files = DailyFiles {
            dir: dir.path().join("logs"),
            component: "connector",
            keep_days: 14,
            open: Mutex::new(None),
        };
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let old = dir
            .path()
            .join("logs")
            .join(format!("connector-{}.log", date_of(today() - 30)));
        let recent = dir
            .path()
            .join("logs")
            .join(format!("connector-{}.log", date_of(today() - 3)));
        let other = dir.path().join("logs").join("worker-1999-01-01.log");
        for path in [&old, &recent, &other] {
            std::fs::write(path, b"before\n").unwrap();
        }
        std::io::Write::write_all(&mut files.make_writer(), b"a line\n").unwrap();
        let written = dir
            .path()
            .join("logs")
            .join(format!("connector-{}.log", date_of(today())));
        assert_eq!(std::fs::read_to_string(written).unwrap(), "a line\n");
        assert!(!old.exists(), "older than the days kept");
        assert!(recent.exists());
        assert!(
            other.exists(),
            "another component's file is not this one's to remove"
        );
    }

    /// Takes all it is given, or what still fits of it while it fills, or nothing once it is full: a disk that
    /// fills up and is then given space again.
    struct FillingDisk {
        written: Vec<u8>,
        room: Option<usize>,
    }

    impl std::io::Write for FillingDisk {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let taken = self.room.map_or(bytes.len(), |room| room.min(bytes.len()));
            if taken == 0 && !bytes.is_empty() {
                return Err(std::io::ErrorKind::StorageFull.into());
            }
            self.written.extend_from_slice(&bytes[..taken]);
            self.room = self.room.map(|room| room - taken);
            Ok(taken)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_line_a_full_disk_cuts_is_the_only_line_lost_and_the_next_begins_on_a_line_of_its_own() {
        let mut disk = FillingDisk {
            written: Vec::new(),
            room: None,
        };
        let mut cut = false;
        write_line(&mut disk, &mut cut, b"first\n").unwrap();
        disk.room = Some(4);
        assert!(write_line(&mut disk, &mut cut, b"second\n").is_err());
        assert!(cut, "only the beginning of the line got there");
        assert!(
            write_line(&mut disk, &mut cut, b"third\n").is_err(),
            "the disk is full"
        );
        disk.room = None;
        write_line(&mut disk, &mut cut, b"fourth\n").unwrap();
        write_line(&mut disk, &mut cut, b"fifth\n").unwrap();
        assert_eq!(
            String::from_utf8(disk.written).unwrap(),
            "first\nseco\nfourth\nfifth\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_file_of_the_day_never_fails_the_formatter_on_a_full_disk() {
        let dir = tempfile::tempdir().unwrap();
        let files = DailyFiles {
            dir: dir.path().to_owned(),
            component: "connector",
            keep_days: 14,
            open: Mutex::new(Some(DayFile {
                date: date_of(today()),
                file: std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
                cut: false,
            })),
        };
        assert!(std::io::Write::write_all(&mut files.make_writer(), b"a line\n").is_ok());
        assert!(std::io::Write::flush(&mut files.make_writer()).is_ok());
    }
}
