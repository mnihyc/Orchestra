//! Measure the resident pages of the deployed executable while clients open and revisit threads.
use super::{Api, Duration, PASSWORD, TestDb, Value, json};
use orch_types::{Clock, SystemClock, Time};
use reqwest::{Method, RequestBuilder, header};

// Twelve openings at two per second make ten rounds per minute.
const ROUNDS_PER_MINUTE: u32 = 10;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

fn status_kib(pid: u32, field: &str) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

async fn create(api: &Api, path: &str, token: &str, body: Value) -> String {
    let (status, reply) = api.send(Method::POST, path, Some(token), Some(body)).await;
    assert_eq!(status, 200, "{path}: {reply}");
    reply["id"].as_str().unwrap().to_owned()
}

struct Session {
    token: String,
    cookie: String,
    expires: Time,
}

impl Session {
    async fn from_request(request: RequestBuilder) -> Self {
        let reply = request
            .timeout(RESPONSE_TIMEOUT)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let cookie = reply
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok()?.split(';').next())
            .find(|pair| pair.starts_with("orch_session="))
            .expect("the API set its browser session cookie")
            .to_owned();
        let body: Value = reply.json().await.unwrap();
        Self {
            token: body["access_token"].as_str().unwrap().to_owned(),
            cookie,
            expires: body["access_expires_at"].as_str().unwrap().parse().unwrap(),
        }
    }
}

async fn create_openings(api: &Api, token: &str) -> Vec<(String, String)> {
    let (status, mode) = api
        .send(
            Method::PUT,
            "/me/admin-mode",
            Some(token),
            Some(json!({"as_admin":true})),
        )
        .await;
    assert_eq!(status, 200, "{mode}");
    let org = create(
        api,
        "/orgs",
        token,
        json!({"name":"Office","slug":"office"}),
    )
    .await;
    let project = create(
        api,
        &format!("/orgs/{org}/projects"),
        token,
        json!({"name":"Project"}),
    )
    .await;
    let topic = create(
        api,
        &format!("/projects/{project}/topics"),
        token,
        json!({"name":"Topic"}),
    )
    .await;
    let mut openings = Vec::new();
    for n in 0..12 {
        let thread = create(
            api,
            &format!("/topics/{topic}/threads"),
            token,
            json!({
                "name":format!("Thread {n}"),"program":{"id":"orch"}
            }),
        )
        .await;
        openings.push((format!("{}/threads/{thread}/opening", api.base), thread));
    }

    openings
}

#[tokio::test(flavor = "multi_thread")]
async fn five_browsers_can_revisit_a_dozen_threads_without_growing_the_api_working_set() {
    measure_browser_working_set(1).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the sustained browser workload for the full 89-minute observation"]
async fn the_api_stays_responsive_and_within_its_warm_memory_bound_for_eighty_nine_minutes() {
    measure_browser_working_set(89).await;
}

async fn measure_browser_working_set(minutes: u32) {
    let database = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let api = Api::start(dir.path(), &database.url, false).await;
    let pid = api.child.id().unwrap();
    let mut session = Session::from_request(
        api.client
            .post(format!("{}/setup", api.base))
            .json(&json!({"username":"ada","display_name":"Ada","password":PASSWORD})),
    )
    .await;
    let openings = create_openings(&api, &session.token).await;

    // The incident had five clients and about two requests per second. Warm all twelve pages
    // once, then restart four connection pools each round while one returning browser stays.
    let mut warmed = None;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    for round in 0..=minutes * ROUNDS_PER_MINUTE {
        // Refresh before expiry while the same browser and server stay up across the trial.
        if session.expires <= SystemClock.time().checked_add_minutes(1).unwrap() {
            session = Session::from_request(
                api.client
                    .post(format!("{}/auth/refresh", api.base))
                    .header(header::COOKIE, session.cookie.as_str())
                    .header("x-orch-csrf", "1"),
            )
            .await;
        }
        if round.is_multiple_of(ROUNDS_PER_MINUTE) {
            let meta = api
                .client
                .get(format!("{}/meta", api.base))
                .timeout(RESPONSE_TIMEOUT)
                .send()
                .await
                .unwrap();
            assert_eq!(meta.status(), 200);
            let body: Value = meta.json().await.unwrap();
            assert!(body.is_object(), "the API returned its public metadata");
        }
        let mut clients = vec![api.client.clone()];
        clients.extend((1..5).map(|_| Api::client(&api.ca)));
        let mut peak = 0;
        for (index, (url, thread)) in openings.iter().enumerate() {
            tick.tick().await;
            let reply = clients[index % clients.len()]
                .get(url)
                .bearer_auth(&session.token)
                .timeout(RESPONSE_TIMEOUT)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
            let opening: Value = reply.json().await.unwrap();
            assert_eq!(
                opening["sync"]["thread"]["id"].as_str(),
                Some(thread.as_str())
            );
            peak = peak.max(status_kib(pid, "VmRSS:"));
        }
        drop(clients);
        let baseline = *warmed.get_or_insert(peak);
        eprintln!("browser round {round}: peak {peak} KiB, warmed {baseline} KiB");
        // Twice the observed live heap leaves room for binary pages, stacks and active requests.
        assert!(peak <= 768 * 1024, "browser workload exceeded 768 MiB RSS");
        // One observed glibc arena of slack across every repeat of the same working set.
        assert!(
            peak <= baseline + 64 * 1024,
            "active RSS grew over 64 MiB after warmup"
        );
    }
    // Keep the returning browser idle through two upkeep ticks before the final reading.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let idle = status_kib(pid, "VmRSS:");
    let high_water = status_kib(pid, "VmHWM:");
    eprintln!(
        "idle browser after the workload: {idle} KiB RSS, process high-water {high_water} KiB"
    );
    // The kernel includes transient peaks while a request was still in flight.
    assert!(
        high_water <= 768 * 1024,
        "process peak exceeded 768 MiB RSS"
    );
    assert!(idle <= warmed.unwrap() + 64 * 1024 && idle <= 768 * 1024);
    api.stop().await;
}
