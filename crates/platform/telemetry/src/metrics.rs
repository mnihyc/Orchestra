//! Metric registration, the exporter for an operator's own collector, and the readings a service samples into the database.
//!
//! Code records with the `metrics` macros. [`install`] keeps what was
//! recorded in the process; [`Readings::render`] writes it in the text
//! format a Prometheus collector reads, for the service's metrics
//! listener to answer with.

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

mod samples;

/// What this process has measured.
#[derive(Debug, Clone)]
pub struct Readings {
    prometheus: PrometheusHandle,
    samples: samples::Samples,
}

impl std::fmt::Debug for samples::Samples {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Samples")
    }
}

static PROCESS: OnceLock<Readings> = OnceLock::new();

/// Why measurements could not be kept.
#[derive(Debug, thiserror::Error)]
#[error("metrics could not be installed: {0}")]
pub struct MetricsError(String);

/// Starts keeping what is recorded in this process. Called once, at start.
pub fn install() -> Result<Readings, MetricsError> {
    let recorder = builder()?.build_recorder();
    let readings = Readings {
        prometheus: recorder.handle(),
        samples: samples::Samples::default(),
    };
    metrics::set_global_recorder(samples::Mirrored {
        inner: recorder,
        samples: readings.samples.clone(),
    })
    .map_err(|_| MetricsError("a metrics recorder is installed already".to_owned()))?;
    let _ = PROCESS.set(readings.clone());
    crate::database::register();
    Ok(readings)
}

fn builder() -> Result<PrometheusBuilder, MetricsError> {
    let speed = orch_types::measurements::model_performance_buckets(1.0);
    // The ledger stores first-output milliseconds, while Prometheus durations use seconds.
    let first = orch_types::measurements::model_performance_buckets(0.001);
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("orch_model_output_tokens_per_second".into()),
            &speed,
        )
        .map_err(|error| MetricsError(error.to_string()))?
        .set_buckets_for_metric(
            Matcher::Full("orch_model_first_token_seconds".into()),
            &first,
        )
        .map_err(|error| MetricsError(error.to_string()))
}

/// Interval aggregates of this process, for its existing internal reporting path; nothing goes to an outside collector.
pub fn take_process_samples() -> Vec<orch_types::measurements::MetricSample> {
    PROCESS.get().map_or_else(Vec::new, |readings| {
        readings.upkeep();
        readings.take_samples()
    })
}

impl Readings {
    /// Everything measured so far, in the Prometheus text format.
    #[must_use]
    pub fn render(&self) -> String {
        // The facade has integer counters only; this one comes from a decimal ledger total.
        self.prometheus.render().replace(
            "# TYPE orch_usage_credits_total gauge",
            "# TYPE orch_usage_credits_total counter",
        )
    }

    /// Lets go of what histograms need only between two renders; called
    /// from time to time by a process that is seldom scraped.
    pub fn upkeep(&self) {
        self.prometheus.run_upkeep();
    }

    /// Takes counter growth, current gauges and bounded histogram means since the previous interval.
    #[must_use]
    pub fn take_samples(&self) -> Vec<orch_types::measurements::MetricSample> {
        self.samples.take()
    }
}

/// The content type of the Prometheus text format.
pub const EXPOSITION_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// The largest request head a measurements listener reads: a collector's request is a few
/// hundred bytes, and nothing else is asked of it.
const REQUEST_HEAD_MAX: usize = 8 * 1024;
/// How long one connection may take to ask and be answered, so that an idle one holds nothing.
const ANSWER_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Answers `GET /metrics` on `listener` with what this process has measured, until `stop`
/// resolves: the operator's collector reads it there, on the service's `metrics_bind`. One request
/// is answered on each connection; another path is answered 404, and a request head of more than
/// 8 KiB 431. It serves the measurements of the process that installed them, for as long as it
/// runs.
pub async fn serve(
    listener: tokio::net::TcpListener,
    readings: Readings,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let mut stop = std::pin::pin!(stop);
    loop {
        let accepted = tokio::select! {
            () = &mut stop => return Ok(()),
            accepted = listener.accept() => accepted,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::debug!(%error, "a connection for the measurements could not be taken");
                continue;
            }
        };
        let readings = readings.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(ANSWER_WITHIN, answer(stream, &readings)).await;
        });
    }
}

/// Reads one request head from `stream` and answers it.
async fn answer(mut stream: tokio::net::TcpStream, readings: &Readings) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt as _;
    let mut head: Vec<u8> = Vec::with_capacity(512);
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|four| four == b"\r\n\r\n") {
        if head.len() > REQUEST_HEAD_MAX {
            return respond(
                &mut stream,
                "431 Request Header Fields Too Large",
                "text/plain; charset=utf-8",
                b"",
            )
            .await;
        }
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buffer[..read]);
    }
    let line = head.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let mut words = line.split(|byte| *byte == b' ');
    let method = words.next().unwrap_or_default();
    let target = words.next().unwrap_or_default();
    let path = target
        .split(|byte| *byte == b'?')
        .next()
        .unwrap_or_default();
    if method == b"GET" && path == b"/metrics" {
        let body = readings.render();
        respond(&mut stream, "200 OK", EXPOSITION_TYPE, body.as_bytes()).await
    } else {
        respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"the measurements are at GET /metrics\n",
        )
        .await
    }
}

/// Writes a whole answer and closes the connection.
async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_histograms_export_the_same_logarithmic_edges_as_usage_in_their_public_units() {
        let recorder = super::builder().unwrap().build_recorder();
        metrics::with_local_recorder(&recorder, || {
            metrics::histogram!("orch_model_first_token_seconds").record(1.0);
            metrics::histogram!("orch_model_output_tokens_per_second").record(32.0);
        });
        let text = recorder.handle().render();
        assert!(text.contains("# TYPE orch_model_first_token_seconds histogram"));
        assert!(text.contains("orch_model_first_token_seconds_bucket{le=\"1.024\"} 1"));
        assert!(text.contains("orch_model_output_tokens_per_second_bucket{le=\"32\"} 1"));
        assert!(text.contains("orch_model_output_tokens_per_second_count 1"));
    }
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// What a listener answers to `request`, as text.
    async fn ask(address: std::net::SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }

    #[tokio::test]
    async fn a_collector_reads_the_measurements_and_nothing_else_is_served() {
        // A handle of its own: the recorder of the process is installed once, by the service.
        let readings = Readings {
            prometheus: PrometheusBuilder::new().build_recorder().handle(),
            samples: samples::Samples::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tell, told) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(serve(listener, readings, async move {
            let _ = told.await;
        }));

        let read = ask(address, "GET /metrics HTTP/1.1\r\nhost: here\r\n\r\n").await;
        assert!(read.starts_with("HTTP/1.1 200 OK\r\n"), "{read}");
        assert!(read.contains(EXPOSITION_TYPE), "{read}");
        let other = ask(address, "GET /other HTTP/1.1\r\nhost: here\r\n\r\n").await;
        assert!(other.starts_with("HTTP/1.1 404 Not Found\r\n"), "{other}");
        let posted = ask(address, "POST /metrics HTTP/1.1\r\nhost: here\r\n\r\n").await;
        assert!(posted.starts_with("HTTP/1.1 404"), "{posted}");

        tell.send(()).unwrap();
        serving.await.unwrap().unwrap();
    }
}
