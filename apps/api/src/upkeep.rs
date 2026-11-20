//! Periodic process maintenance runs independently of browser requests.
use orch_telemetry::metrics::Readings;

pub(super) async fn run(readings: &Readings) {
    readings.upkeep();
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if let Err(error) = tokio::task::spawn_blocking(release_free_pages).await {
        tracing::debug!(%error, "heap upkeep ended early");
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(
    unsafe_code,
    reason = "glibc exposes page reclamation through its C interface"
)]
fn release_free_pages() {
    // SAFETY: malloc_trim is thread-safe and takes a byte count, with no pointer or lifetime
    // preconditions. Zero asks glibc to return every available whole page in its free chunks.
    unsafe { libc::malloc_trim(0) };
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    fn resident_kib() -> u64 {
        let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap();
        rollup
            .lines()
            .find_map(|line| line.strip_prefix("Rss:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn idle_clients_can_keep_their_state_without_pinning_freed_request_pages() {
        let readings = orch_telemetry::install_metrics().unwrap();
        // Interleave small surviving buffers with a total of 64 MiB of request data.
        // The small allocations keep the surrounding arenas live after the requests finish.
        let mut requests = Vec::with_capacity(1024);
        let mut clients = Vec::with_capacity(1024);
        for _ in 0..1024 {
            requests.push(vec![0xa5_u8; 64 * 1024]);
            clients.push(vec![0x5a_u8; 2048]);
        }
        std::hint::black_box(&requests);
        let active = resident_kib();
        drop(requests);
        super::run(&readings).await;
        let idle = resident_kib();
        eprintln!("request buffers active {active} KiB, idle {idle} KiB");
        // Interior whole pages account for well over three quarters of each freed buffer.
        assert!(
            idle + 48 * 1024 <= active,
            "free request pages stayed resident"
        );
        assert!(
            clients
                .iter()
                .all(|client| client.iter().all(|&byte| byte == 0x5a))
        );
    }
}
