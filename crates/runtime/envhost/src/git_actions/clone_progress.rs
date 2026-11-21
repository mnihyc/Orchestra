//! Read Git's local progress and libcurl's receive sizes. Only numbers leave the reader.

use std::time::Duration;

use orch_api_contract::workspace::JobProgress;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::Instant;

use crate::error::{HostError, Result};

#[derive(Default)]
pub(super) struct Counters {
    pub(super) bytes: i64,
    pub(super) objects: i64,
    total: Option<i64>,
    http_bytes: i64,
    complete: bool,
}

impl Counters {
    fn observe(&mut self, line: &str) -> bool {
        // Git timestamps its own curl records. The following lines contain headers or data
        // and are discarded, even when their contents look like another receive record.
        if let Some((stamp, rest)) = line.split_once(" http.c:")
            && stamp.len() == "00:00:00.000000".len()
            && let Some((source_line, metadata)) = rest.split_once(char::is_whitespace)
            && source_line.bytes().all(|c| c.is_ascii_digit())
            && let Some(size) = metadata.trim_start().strip_prefix("<= Recv data, ")
            && let Some(size) = size
                .split_once(" bytes ")
                .and_then(|(n, _)| n.parse::<i64>().ok())
            && size > 0
        {
            self.http_bytes = self.http_bytes.saturating_add(size);
            self.bytes = self.bytes.max(self.http_bytes);
            return true;
        }
        let Some(row) = line.strip_prefix("Receiving objects:") else {
            return false;
        };
        let Some((counts, tail)) = row.split_once('(').and_then(|(_, row)| row.split_once(')'))
        else {
            return false;
        };
        let Some((objects, total)) = counts
            .split_once('/')
            .and_then(|(n, all)| Some((n.parse::<i64>().ok()?, all.parse::<i64>().ok()?)))
        else {
            return false;
        };
        if total <= 0 || !(0..=total).contains(&objects) {
            return false;
        }
        let before = (self.objects, self.bytes);
        self.objects = self.objects.max(objects);
        self.total = Some(total);
        if self.http_bytes == 0
            && let Some(amount) = tail
                .trim_start_matches(',')
                .trim()
                .split_once(" | ")
                .map(|(amount, _)| amount)
            && let Some((number, unit)) = amount.split_once(' ')
            && let Ok(number) = number.parse::<f64>()
        {
            let scale: f64 = match unit {
                "bytes" => 1.,
                "KiB" => 1024.,
                "MiB" => 1048576.,
                "GiB" => 1073741824.,
                _ => 0.,
            };
            self.bytes = self.bytes.max((number * scale) as i64);
        }
        self.complete |= objects == total && tail.contains("done.");
        before != (self.objects, self.bytes)
    }

    fn report(&self) {
        if let Some(sink) = crate::snapshot::current_progress() {
            sink.report(&JobProgress {
                activity: None,
                phase: "apply".into(),
                phases: Vec::new(),
                percent: self.total.map_or(0, |all| {
                    ((i128::from(self.objects) * 100) / i128::from(all)) as u32
                }),
                files_done: self.objects,
                files_total: self.total,
                bytes_done: self.bytes,
                bytes_total: None,
                rate_bps: None,
                eta_seconds: None,
                message: None,
            });
        }
    }
}

pub(super) async fn read_until_stall(
    stream: &mut (impl AsyncRead + Unpin),
    idle: Duration,
    counters: &mut Counters,
) -> Result<Vec<u8>> {
    // Diagnostic classification needs only Git's own fatal/error lines. Never retain curl
    // payload or headers. Bound an unterminated record as well as the kept diagnostics.
    let mut errors = Vec::new();
    let mut line = Vec::new();
    let mut truncated = false;
    let mut buf = [0; 8192];
    let mut last_change = Instant::now();
    let mut last_report = None;
    counters.report();
    loop {
        let deadline = last_change.checked_add(idle);
        let count = if let Some(due) = deadline.filter(|_| !counters.complete) {
            tokio::time::timeout_at(due, stream.read(&mut buf))
                .await
                .map_err(|_| {
                    counters.report();
                    HostError::CloneStalled(idle.as_secs())
                })?
        } else {
            stream.read(&mut buf).await
        }
        .map_err(|e| HostError::io("reading repository progress", e))?;
        if count == 0 {
            counters.report();
            return Ok(errors);
        }
        for &byte in &buf[..count] {
            if matches!(byte, b'\n' | b'\r') {
                if !truncated && !line.is_empty() {
                    if counters.observe(std::str::from_utf8(&line).unwrap_or("")) {
                        last_change = Instant::now();
                        if last_report.is_none_or(|last| {
                            last_change - last >= crate::snapshot::PROGRESS_INTERVAL
                        }) {
                            counters.report();
                            last_report = Some(last_change);
                        }
                    } else if (line.starts_with(b"fatal: ") || line.starts_with(b"error: "))
                        && errors.len() + line.len() < super::ERROR_KEEP_BYTES as usize
                    {
                        errors.extend_from_slice(&line);
                        errors.push(b'\n');
                    }
                }
                line.clear();
                truncated = false;
            } else if line.len() < super::ERROR_KEEP_BYTES as usize {
                line.push(byte);
            } else {
                truncated = true;
            }
        }
        // An always-readable stream of unchanged progress cannot postpone expiration.
        if !counters.complete && last_change.elapsed() >= idle {
            counters.report();
            return Err(HostError::CloneStalled(idle.as_secs()));
        }
    }
}
