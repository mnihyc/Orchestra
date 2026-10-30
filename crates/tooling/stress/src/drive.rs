//! The drive: for a set time, every thread takes turns at once through the scripted harness, simulated browsers
//! read pages and hold live feeds, and people search and use the finder's name search.
//!
//! - A thread worker sends an input (a `#!fake` script that thinks, plans, runs, edits and answers, with pauses that
//!   stand for a model's time), waits on its person's socket for the turn to end, pauses as a person does, and sends
//!   the next. Each person keeps one socket that follows all their threads, as a tab does.
//! - A browser is a person with a socket of their own: it opens a page (all the reads the web application makes for
//!   it, at once), follows the page's feed while it reads, and goes on to another page. Now and then it writes in a
//!   document (a block, or an answer in a discussion) and types a name into the finder.
//! - A searcher searches the words the documents and turns are written with, and types names into the finder.

use crate::api::{Api, Person, Reply, query};
use crate::feed::{self, Books, Socket};
use crate::rng::Rng;
use crate::seed::WORDS;
use crate::state::State;
use crate::statements;
use futures::future::join_all;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// How the drive runs.
pub struct DriveOptions {
    pub duration: Duration,
    pub browsers: usize,
    pub searchers: usize,
    /// How long a turn's script pauses in all (the time a model would take), in seconds.
    pub turn_seconds: f64,
    /// How long a person pauses between a turn's end and their next input, on average.
    pub pause: Duration,
    /// How long a person reads a page, on average.
    pub dwell: Duration,
    pub turn_timeout: Duration,
    pub password: String,
    pub ws: String,
    /// Whether the thread workers run (off: only browsers and searchers).
    pub turns: bool,
    /// The database whose `pg_stat_statements` is read when the window opens and closes.
    pub database_url: Option<String>,
    /// The threshold of the statements reported, in milliseconds.
    pub slow_ms: f64,
    /// Whether a thread blocked on its provider makes the administrator refresh that provider.
    pub refresh_blocked: bool,
}

type Shared = Arc<Ctx>;

struct Ctx {
    api: Arc<Api>,
    books: Arc<Books>,
    state: Arc<State>,
    people: Vec<Arc<Person>>,
    admin: Arc<Person>,
    stop: CancellationToken,
    o: DriveOptions,
    /// What each org holds, by index: projects, topics, threads, documents.
    by_org: Vec<OrgIndex>,
    /// When each provider was last refreshed by the drive.
    refreshed: std::sync::Mutex<std::collections::HashMap<String, Instant>>,
}

#[derive(Default, Clone)]
struct OrgIndex {
    projects: Vec<usize>,
    topics: Vec<usize>,
    threads: Vec<usize>,
    docs: Vec<usize>,
}

/// Sleeps, or returns false at once when the drive stops.
async fn rest(stop: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        () = stop.cancelled() => false,
        () = tokio::time::sleep(d) => true,
    }
}

/// Runs the drive; returns when the window has passed and the tasks have stopped.
/// What the database said of the window, when it was given: its long statements and its sessions.
pub struct Database {
    pub slow: Option<Vec<statements::Slow>>,
    pub activity: Option<statements::Activity>,
}

pub async fn drive(
    api: Arc<Api>,
    books: Arc<Books>,
    state: Arc<State>,
    admin: Arc<Person>,
    o: DriveOptions,
) -> anyhow::Result<Database> {
    let rec = api.rec.clone();
    eprintln!("signing in {} people", state.people.len());
    let signed = join_all(
        state
            .people
            .iter()
            .map(|p| api.sign_in(&p.login, &o.password)),
    )
    .await;
    let mut people = Vec::new();
    for (rec_p, person) in state.people.iter().zip(signed) {
        match person {
            Ok(person) => people.push(person),
            Err(e) => anyhow::bail!("{} cannot sign in: {e}", rec_p.login),
        }
    }
    let mut by_org = vec![OrgIndex::default(); state.orgs.len()];
    for (i, p) in state.projects.iter().enumerate() {
        by_org[p.org].projects.push(i);
    }
    for (i, t) in state.topics.iter().enumerate() {
        by_org[t.org].topics.push(i);
    }
    for (i, t) in state.threads.iter().enumerate() {
        by_org[t.org].threads.push(i);
    }
    for (i, d) in state.docs.iter().enumerate() {
        by_org[d.org].docs.push(i);
    }
    let stop = CancellationToken::new();
    let ctx: Shared = Arc::new(Ctx {
        api,
        books,
        state,
        people,
        admin,
        stop: stop.clone(),
        o,
        by_org,
        refreshed: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let mut tasks = JoinSet::new();
    // The thread workers' sockets live as long as the drive: a socket whose last handle is dropped closes.
    let mut work_sockets: Vec<Socket> = Vec::new();

    if ctx.o.turns {
        // One socket per person that works in threads (the administrator's for the threads the administrator made).
        let mut owners: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for (i, t) in ctx.state.threads.iter().enumerate() {
            owners.entry(t.owner).or_default().push(i);
        }
        for (owner, threads) in owners {
            let person = owner.map_or_else(|| ctx.admin.clone(), |p| ctx.people[p].clone());
            let socket = feed::open(
                ctx.o.ws.clone(),
                person.clone(),
                ctx.books.clone(),
                stop.clone(),
            );
            for (k, t) in threads.iter().enumerate() {
                socket.sub(
                    k as i64 + 1,
                    &format!("thread:{}", ctx.state.threads[*t].id),
                );
            }
            for t in threads {
                tasks.spawn(thread_worker(ctx.clone(), t, person.clone()));
            }
            work_sockets.push(socket);
        }
    }
    for b in 0..ctx.o.browsers.min(ctx.people.len()) {
        tasks.spawn(browser(ctx.clone(), b));
    }
    for s in 0..ctx.o.searchers {
        tasks.spawn(searcher(ctx.clone(), s));
    }

    // The sockets connect and the first pages load before the window opens.
    tokio::time::sleep(Duration::from_secs(5)).await;
    eprintln!(
        "{} the measured window opens: {} s, {} thread workers, {} browsers, {} searchers",
        chrono::Utc::now().format("%H:%M:%S"),
        ctx.o.duration.as_secs(),
        if ctx.o.turns {
            ctx.state.threads.len()
        } else {
            0
        },
        ctx.o.browsers.min(ctx.people.len()),
        ctx.o.searchers
    );
    let before = match ctx.o.database_url.as_deref() {
        Some(url) => match statements::snapshot(url).await {
            Ok(s) => Some(s),
            Err(e) => {
                rec.note(format!("the statements cannot be read: {e:#}"));
                None
            }
        },
        None => None,
    };
    // The database's sessions, sampled every two seconds through the window (one connection of its own).
    let sampling = CancellationToken::new();
    let sampler = ctx.o.database_url.clone().map(|url| {
        tokio::spawn(statements::sample_activity(
            url,
            Duration::from_secs(2),
            sampling.clone(),
        ))
    });
    rec.set_measuring(true);
    let window = Instant::now();
    let mut last = Instant::now();
    while window.elapsed() < ctx.o.duration {
        tokio::time::sleep(
            Duration::from_secs(1).min(ctx.o.duration.saturating_sub(window.elapsed())),
        )
        .await;
        if last.elapsed() >= Duration::from_secs(60) {
            last = Instant::now();
            let m = rec.report();
            let n: usize = m.routes.iter().map(|r| r.n).sum();
            let errors: u64 = m.routes.iter().flat_map(|r| r.errors.values()).sum();
            eprintln!(
                "{} {:.0} s: {n} requests, {errors} failed, {} turns sent, {} ended",
                chrono::Utc::now().format("%H:%M:%S"),
                window.elapsed().as_secs_f64(),
                m.turns.sent,
                m.turns.ended.values().sum::<u64>()
            );
        }
    }
    rec.set_measuring(false);
    sampling.cancel();
    eprintln!(
        "{} the window closed; stopping",
        chrono::Utc::now().format("%H:%M:%S")
    );
    let activity = match sampler {
        Some(task) => match task.await {
            Ok(Ok(activity)) => Some(activity),
            Ok(Err(e)) => {
                rec.note(format!(
                    "the database's sessions could not be sampled: {e:#}"
                ));
                None
            }
            Err(_) => None,
        },
        None => None,
    };
    let slow = match (before, ctx.o.database_url.as_deref()) {
        (Some(before), Some(url)) => match statements::snapshot(url).await {
            Ok(after) => Some(statements::slow(&before, &after, ctx.o.slow_ms)),
            Err(e) => {
                rec.note(format!("the statements cannot be read at the end: {e:#}"));
                None
            }
        },
        _ => None,
    };
    stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(20), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    drop(work_sockets);
    Ok(Database { slow, activity })
}

/// The input of one turn: a script of the scripted harness that does what an agent's turn does, with pauses that
/// stand for a model's time.
fn script(n: u64, thread: &str, rng: &mut Rng, turn_seconds: f64) -> String {
    let (a, b) = (WORDS[rng.below(WORDS.len())], WORDS[rng.below(WORDS.len())]);
    let pause = (turn_seconds * 1000.0 / 3.0).max(0.0) as u64;
    format!(
        "#!fake\nthink Reading request {n} of {thread}: {a} and {b}\nplan Read | Change | Check\n\
         run git status --short => 0  M src/{a}.rs\nsleep {pause}\nedit src/{a}.rs +12 -3\nsleep {pause}\n\
         run cargo test --quiet => 0 test result: ok. 42 passed; 0 failed\nsleep {pause}\nusage 12000 800\n\
         say Request {n} is done: {a} and {b} are in place and the tests pass."
    )
}

async fn thread_worker(ctx: Shared, t: usize, person: Arc<Person>) {
    let thread = ctx.state.threads[t].clone();
    let rec = ctx.api.rec.clone();
    let mut rng = Rng::new(t as u64 + 1000);
    // Not all at once: people start their days apart.
    if !rest(&ctx.stop, rng.around(ctx.o.pause)).await {
        return;
    }
    let mut n = 0;
    while !ctx.stop.is_cancelled() {
        n += 1;
        let client_id = format!("stress-{}", uuid::Uuid::new_v4());
        let ended = ctx.books.turns.expect(&thread.id, &client_id);
        ctx.books.lag.mark(&client_id);
        let sent = Instant::now();
        let body = json!({
            "content": [{ "type": "text", "text": script(n, &thread.name, &mut rng, ctx.o.turn_seconds) }],
            "delivery": "queue", "client_id": client_id,
        });
        let reply = ctx
            .api
            .post(&person, "/threads/{id}/inputs", &[&thread.id], &body)
            .await;
        if !reply.ok() {
            ctx.books.turns.forget(&client_id);
            rec.turn_refused(format!(
                "{} {}",
                reply.status,
                reply.text("/code").unwrap_or_default()
            ));
            if !rest(&ctx.stop, ctx.o.pause * 3).await {
                return;
            }
            continue;
        }
        rec.turn_sent();
        let input = reply.text("/input/id").unwrap_or_default();
        ctx.books.turns.input_known(&client_id, &input);
        // The same input is waited on until its turn ends: a thread that says it is blocked is not sent more
        // (that would only queue them), and is counted each time it says so.
        let deadline = tokio::time::Instant::now() + ctx.o.turn_timeout;
        let mut ended = ended;
        loop {
            let outcome = tokio::select! {
                () = ctx.stop.cancelled() => { ctx.books.turns.forget(&client_id); return; }
                r = tokio::time::timeout_at(deadline, ended) => r,
            };
            match outcome {
                Ok(Ok(end)) if end.state == "blocked" => {
                    let why = end.code.unwrap_or_default();
                    rec.turn_blocked(why.clone());
                    if ctx.o.refresh_blocked && why.contains("provider") {
                        refresh_provider(&ctx, &thread).await;
                    }
                    ended = ctx.books.turns.expect(&thread.id, &client_id);
                    ctx.books.turns.input_known(&client_id, &input);
                }
                Ok(Ok(end)) => {
                    let how = match &end.code {
                        Some(code) => format!("{} ({code})", end.state),
                        None => end.state.clone(),
                    };
                    rec.turn_ended(how, sent.elapsed(), end.state == "completed");
                    break;
                }
                Ok(Err(_)) => {
                    ctx.books.turns.forget(&client_id);
                    break;
                }
                Err(_) => {
                    ctx.books.turns.forget(&client_id);
                    rec.turn_timed_out();
                    let now = ctx
                        .api
                        .get(
                            &person,
                            "/threads/{id}/sync",
                            &[&thread.id],
                            &query(&[("limit", "1")]),
                        )
                        .await;
                    rec.note(format!(
                        "no end of a turn of {} was seen in {} s; the thread is {} ({})",
                        thread.name,
                        ctx.o.turn_timeout.as_secs(),
                        now.text("/thread/state").unwrap_or_else(|| now.why()),
                        now.body
                            .pointer("/thread/blocked")
                            .map(Value::to_string)
                            .unwrap_or_default()
                    ));
                    break;
                }
            }
        }
        if !rest(&ctx.stop, rng.around(ctx.o.pause)).await {
            return;
        }
    }
}

/// Asks the provider of a thread to be looked at again (`--refresh-blocked`), as its manager does with the
/// provider's Refresh: at most once in 30 s for one provider, by the administrator.
async fn refresh_provider(ctx: &Ctx, thread: &crate::state::ThreadRec) {
    let provider = thread
        .provider
        .clone()
        .or_else(|| ctx.state.orgs[thread.org].providers.first().cloned());
    let Some(provider) = provider else { return };
    {
        let mut last = ctx
            .refreshed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last
            .get(&provider)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        last.insert(provider.clone(), Instant::now());
    }
    ctx.api.rec.refreshed();
    ctx.api
        .post(
            &ctx.admin,
            "/providers/{id}/refresh",
            &[&provider],
            &json!({}),
        )
        .await;
}

type Call<'a> = Pin<Box<dyn Future<Output = Reply> + Send + 'a>>;

/// Opens a page: its reads at once, timed until the last answered.
async fn page(ctx: &Ctx, kind: &str, calls: Vec<Call<'_>>) -> Vec<Reply> {
    let started = Instant::now();
    let replies = join_all(calls).await;
    ctx.api.rec.page(kind, started.elapsed());
    replies
}

/// The pages a browser goes to, and how often (out of 100): what a team spends its day on.
const PAGES: [(&str, u32); 8] = [
    ("thread", 30),
    ("document", 20),
    ("topic", 15),
    ("project", 10),
    ("org", 8),
    ("home", 7),
    ("inbox", 5),
    ("search", 5),
];

fn choose(rng: &mut Rng) -> &'static str {
    let mut roll = rng.below(100) as u32;
    for (kind, weight) in PAGES {
        if roll < weight {
            return kind;
        }
        roll -= weight;
    }
    "home"
}

async fn browser(ctx: Shared, b: usize) {
    let person = ctx.people[b].clone();
    let me = &ctx.state.people[b];
    let mut rng = Rng::new(b as u64 + 1);
    let socket = feed::open(
        ctx.o.ws.clone(),
        person.clone(),
        ctx.books.clone(),
        ctx.stop.clone(),
    );
    socket.sub(1, "user");
    let mut sub_id = 10;
    let mut page_sub: Option<i64> = None;
    let mut org_sub: Option<i64> = None;
    if !rest(&ctx.stop, rng.around(Duration::from_secs(2))).await {
        return;
    }
    while !ctx.stop.is_cancelled() {
        let Some(&org) = rng.pick(&me.orgs) else {
            ctx.api.rec.note(format!(
                "{} is a member of no org: the browser only opens home",
                person.login
            ));
            home(&ctx, &person).await;
            if !rest(&ctx.stop, rng.around(ctx.o.dwell)).await {
                return;
            }
            continue;
        };
        let kind = choose(&mut rng);
        if let Some(id) = page_sub.take() {
            socket.unsub(id);
        }
        sub_id += 1;
        let follow = visit(&ctx, &person, org, kind, &mut rng, &socket, sub_id).await;
        if follow {
            page_sub = Some(sub_id);
        }
        // The org's feed is followed while the person is in the org (the tree in the sidebar).
        if kind == "org" {
            if let Some(id) = org_sub.take() {
                socket.unsub(id);
            }
            sub_id += 1;
            socket.sub(sub_id, &format!("org:{}", ctx.state.orgs[org].id));
            org_sub = Some(sub_id);
        }
        if rng.chance(0.2) {
            finder(&ctx, &person, org, &mut rng).await;
        }
        if !rest(&ctx.stop, rng.around(ctx.o.dwell)).await {
            return;
        }
    }
}

async fn home(ctx: &Ctx, who: &Person) {
    let api = &ctx.api;
    page(
        ctx,
        "home",
        vec![
            Box::pin(api.get(who, "/me", &[], "")),
            Box::pin(api.get(who, "/me/state", &[], "")),
            Box::pin(api.get(who, "/orgs", &[], "")),
            Box::pin(api.get(who, "/notifications/summary", &[], "")),
            Box::pin(api.get(who, "/me/favorites", &[], "")),
        ],
    )
    .await;
}

/// Opens one page of `kind` in `org`; returns whether it follows a feed under `sub_id`.
async fn visit(
    ctx: &Ctx,
    who: &Person,
    org: usize,
    kind: &str,
    rng: &mut Rng,
    socket: &Socket,
    sub_id: i64,
) -> bool {
    let api = &ctx.api;
    let state = &ctx.state;
    let index = &ctx.by_org[org];
    match kind {
        "thread" => {
            let Some(&t) = rng.pick(&index.threads) else {
                return false;
            };
            let id = state.threads[t].id.as_str();
            let feed = format!("thread:{id}");
            socket.sub(sub_id, &feed);
            socket.visible(&feed);
            let one = [id];
            let last_hundred = query(&[("limit", "100")]);
            let health = state.threads[t]
                .provider
                .as_deref()
                .map(|p| query(&[("provider_id", p)]));
            let mut reads: Vec<Call<'_>> = vec![
                Box::pin(api.get(who, "/nodes/{id}", &one, "")),
                Box::pin(api.get(who, "/threads/{id}/sync", &one, &last_hundred)),
                Box::pin(api.get(who, "/threads/{id}/parameters", &one, "")),
                Box::pin(api.get(who, "/threads/{id}/limits", &one, "")),
                Box::pin(api.get(who, "/threads/{id}/outline", &one, "")),
            ];
            // The composer's draft is the writer's own: the page asks for it where the person may write.
            if state.threads[t]
                .owner
                .is_some_and(|o| state.people[o].login == who.login)
            {
                reads.push(Box::pin(api.get(who, "/threads/{id}/draft", &one, "")));
            }
            // The status bar's provider health (the page asks it again every 10 s while it stays open).
            if let Some(health) = health.as_deref() {
                reads.push(Box::pin(api.get(who, "/usage/health", &[], health)));
            }
            page(ctx, "thread", reads).await;
            true
        }
        "document" => {
            let Some(&d) = rng.pick(&index.docs) else {
                return false;
            };
            let id = state.docs[d].id.as_str();
            let feed = format!("doc:{id}");
            socket.sub(sub_id, &feed);
            socket.visible(&feed);
            let replies = page(
                ctx,
                "document",
                vec![
                    Box::pin(api.get(who, "/documents/{id}", &[id], "")),
                    Box::pin(api.get(who, "/documents/{id}/sync", &[id], "")),
                    Box::pin(api.get(who, "/documents/{id}/discussions", &[id], "")),
                ],
            )
            .await;
            if rng.chance(0.15) {
                write_block(ctx, who, id, rng).await;
            } else if rng.chance(0.06) {
                answer(ctx, who, id, replies.get(1), replies.get(2)).await;
            }
            true
        }
        "topic" => {
            let Some(&t) = rng.pick(&index.topics) else {
                return false;
            };
            let id = state.topics[t].id.as_str();
            let feed = format!("topic:{id}");
            socket.sub(sub_id, &feed);
            socket.visible(&feed);
            page(
                ctx,
                "topic",
                vec![
                    Box::pin(api.get(who, "/nodes/{id}", &[id], "")),
                    Box::pin(api.get(who, "/topics/{id}/sync", &[id], "")),
                ],
            )
            .await;
            true
        }
        "project" => {
            let Some(&p) = rng.pick(&index.projects) else {
                return false;
            };
            let id = state.projects[p].id.as_str();
            page(
                ctx,
                "project",
                vec![
                    Box::pin(api.get(who, "/nodes/{id}", &[id], "")),
                    Box::pin(api.get(
                        who,
                        "/nodes/{id}/children",
                        &[id],
                        &query(&[("kind", "topic")]),
                    )),
                    Box::pin(api.get(
                        who,
                        "/nodes/{id}/documents",
                        &[id],
                        &query(&[("limit", "50")]),
                    )),
                    Box::pin(api.get(who, "/projects/{id}/topics", &[id], "")),
                ],
            )
            .await;
            false
        }
        "org" => {
            let id = state.orgs[org].id.as_str();
            page(
                ctx,
                "org",
                vec![
                    Box::pin(api.get(who, "/nodes/{id}", &[id], "")),
                    Box::pin(api.get(who, "/orgs/{id}/tree", &[id], "")),
                    Box::pin(api.get(
                        who,
                        "/nodes/{id}/children",
                        &[id],
                        &query(&[("kind", "project")]),
                    )),
                ],
            )
            .await;
            false
        }
        "inbox" => {
            page(
                ctx,
                "inbox",
                vec![
                    Box::pin(api.get(who, "/notifications", &[], &query(&[("limit", "50")]))),
                    Box::pin(api.get(who, "/notifications/summary", &[], "")),
                    Box::pin(api.get(who, "/approvals", &[], "")),
                ],
            )
            .await;
            false
        }
        "search" => {
            let word = WORDS[rng.below(WORDS.len())];
            page(
                ctx,
                "search",
                vec![Box::pin(api.get(
                    who,
                    "/search",
                    &[],
                    &query(&[("q", word), ("limit", "20")]),
                ))],
            )
            .await;
            false
        }
        _ => {
            home(ctx, who).await;
            false
        }
    }
}

/// A person adds a paragraph at the top of a document.
async fn write_block(ctx: &Ctx, who: &Person, doc: &str, rng: &mut Rng) {
    let block = uuid::Uuid::new_v4().to_string();
    let text = format!(
        "A line from {} about {}.",
        who.login,
        WORDS[rng.below(WORDS.len())]
    );
    let body = json!({
        "client_id": uuid::Uuid::new_v4().to_string(),
        "ops": [{ "op": "insert", "block": { "id": block, "type": "paragraph", "content": [{ "type": "text", "text": text }] }, "parent_id": null, "after_id": null }],
    });
    ctx.books.lag.mark(&block);
    ctx.api
        .post(who, "/documents/{id}/ops", &[doc], &body)
        .await;
}

/// A person answers in the document's first discussion, or opens one on its first block.
async fn answer(
    ctx: &Ctx,
    who: &Person,
    doc: &str,
    sync: Option<&Reply>,
    discussions: Option<&Reply>,
) {
    let content = json!([{ "type": "text", "text": format!("Seen by {}.", who.login) }]);
    let client_id = uuid::Uuid::new_v4().to_string();
    if let Some(discussion) = discussions.and_then(|d| d.text("/items/0/id")) {
        let body = json!({ "content": content, "client_id": client_id });
        ctx.api
            .post(
                who,
                "/documents/{id}/discussions/{discussion_id}/comments",
                &[doc, &discussion],
                &body,
            )
            .await;
    } else if let Some(block) = sync.and_then(|s| s.text("/blocks/0/id")) {
        let body = json!({ "block_id": block, "content": content, "client_id": client_id });
        ctx.api
            .post(who, "/documents/{id}/discussions", &[doc], &body)
            .await;
    }
}

/// A name the person may look for in the org, and how to open what it names.
fn target(ctx: &Ctx, org: usize, rng: &mut Rng) -> Option<(String, &'static str, String)> {
    let index = &ctx.by_org[org];
    let state = &ctx.state;
    match rng.below(4) {
        0 => rng.pick(&index.threads).map(|&t| {
            (
                state.threads[t].name.clone(),
                "/nodes/{id}",
                state.threads[t].id.clone(),
            )
        }),
        1 => rng.pick(&index.docs).map(|&d| {
            (
                state.docs[d].title.clone(),
                "/documents/{id}",
                state.docs[d].id.clone(),
            )
        }),
        2 => rng.pick(&index.projects).map(|&p| {
            (
                state.projects[p].name.clone(),
                "/nodes/{id}",
                state.projects[p].id.clone(),
            )
        }),
        _ => rng.pick(&index.topics).map(|&t| {
            (
                state.topics[t].name.clone(),
                "/nodes/{id}",
                state.topics[t].id.clone(),
            )
        }),
    }
}

/// The fewest letters the finder asks the names for: the API refuses fewer (its `SEARCH_QUERY_MIN_CHARS`), and the
/// web application's finder answers one letter by itself, without asking.
const NAME_QUERY_MIN_CHARS: usize = 2;

/// The finder: a person types a name a letter at a time (each letter from the second asks), then opens what they
/// found.
async fn finder(ctx: &Ctx, who: &Person, org: usize, rng: &mut Rng) {
    let Some((name, open, id)) = target(ctx, org, rng) else {
        return;
    };
    let typed: Vec<char> = name.chars().collect();
    let longest = typed.len().min(2 + rng.below(5));
    for n in 1..=longest {
        // A person types a letter every 120 to 250 ms; the finder asks on each one it may ask on.
        if n >= NAME_QUERY_MIN_CHARS {
            let q: String = typed[..n].iter().collect();
            ctx.api
                .get(
                    who,
                    "/search/names",
                    &[],
                    &query(&[("q", &q), ("limit", "8")]),
                )
                .await;
        }
        tokio::time::sleep(Duration::from_millis(120 + rng.below(130) as u64)).await;
    }
    // What was found is opened: its own read, timed as a page (the typing is a person's time, not the system's).
    let started = Instant::now();
    ctx.api.get(who, open, &[&id], "").await;
    ctx.api
        .rec
        .page("finder: opening what it found", started.elapsed());
}

async fn searcher(ctx: Shared, s: usize) {
    if ctx.people.is_empty() {
        return;
    }
    let p = s % ctx.people.len();
    let person = ctx.people[p].clone();
    let orgs = ctx.state.people[p].orgs.clone();
    let mut rng = Rng::new(s as u64 + 5000);
    while !ctx.stop.is_cancelled() {
        let word = WORDS[rng.below(WORDS.len())];
        let scope = rng.pick(&orgs).map(|o| ctx.state.orgs[*o].id.clone());
        let mut pairs = vec![("q", word), ("limit", "20")];
        if rng.chance(0.5)
            && let Some(scope) = scope.as_deref()
        {
            pairs.push(("scope", scope));
        }
        page(
            &ctx,
            "search",
            vec![Box::pin(ctx.api.get(
                &person,
                "/search",
                &[],
                &query(&pairs),
            ))],
        )
        .await;
        if let Some(&org) = rng.pick(&orgs) {
            finder(&ctx, &person, org, &mut rng).await;
        }
        if !rest(&ctx.stop, rng.around(Duration::from_secs(3))).await {
            return;
        }
    }
}
