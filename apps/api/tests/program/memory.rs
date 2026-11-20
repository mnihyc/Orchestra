//! Measure the resident pages of the deployed executable while clients open and revisit threads.
use super::{Api, Duration, PASSWORD, TestDb, Value, json};
use reqwest::Method;

fn resident_kib(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
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

#[tokio::test(flavor = "multi_thread")]
async fn five_browsers_can_revisit_a_dozen_threads_without_growing_the_api_working_set() {
    let database = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let api = Api::start(dir.path(), &database.url, false).await;
    let pid = api.child.id().unwrap();
    let (status, setup) = api
        .send(
            Method::POST,
            "/setup",
            None,
            Some(json!({
                "username":"ada","display_name":"Ada","password":PASSWORD
            })),
        )
        .await;
    assert_eq!(status, 200);
    let token = setup["access_token"].as_str().unwrap();
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
        &api,
        "/orgs",
        token,
        json!({"name":"Office","slug":"office"}),
    )
    .await;
    let project = create(
        &api,
        &format!("/orgs/{org}/projects"),
        token,
        json!({"name":"Project"}),
    )
    .await;
    let topic = create(
        &api,
        &format!("/projects/{project}/topics"),
        token,
        json!({"name":"Topic"}),
    )
    .await;
    let mut openings = Vec::new();
    for n in 0..12 {
        let thread = create(
            &api,
            &format!("/topics/{topic}/threads"),
            token,
            json!({
                "name":format!("Thread {n}"),"program":{"id":"orch"}
            }),
        )
        .await;
        openings.push((format!("{}/threads/{thread}/opening", api.base), thread));
    }

    // The incident had five clients and about two requests per second. Warm all twelve pages
    // once, then restart four connection pools each round while one returning browser stays.
    let mut warmed = None;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Ten measured rounds keep these browsers opening pages for a full minute after warmup.
    for round in 0..=10 {
        let mut clients = vec![api.client.clone()];
        clients.extend((1..5).map(|_| Api::client(&api.ca)));
        let mut peak = 0;
        for (index, (url, thread)) in openings.iter().enumerate() {
            tick.tick().await;
            let reply = clients[index % clients.len()]
                .get(url)
                .bearer_auth(token)
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
            peak = peak.max(resident_kib(pid));
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
    let idle = resident_kib(pid);
    eprintln!("idle browser after the workload: {idle} KiB RSS");
    assert!(idle <= warmed.unwrap() + 64 * 1024 && idle <= 768 * 1024);
    api.stop().await;
}
