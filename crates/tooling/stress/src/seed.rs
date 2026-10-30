//! The seeder: makes, through the HTTP API alone, the orgs, people, environments, providers, agent profiles,
//! projects, topics, threads and documents the drive needs, up to the figures asked for. What the state file says
//! exists is kept; only the difference is made, so a run at a larger scale grows the same world.
//!
//! What it cannot make through the API it says: a host is a machine whose node enrolled (`orch-node enroll`), and a
//! machine is one whose connector enrolled; neither is made by a request.

use crate::Figures;
use crate::api::{Api, Person, query};
use crate::rng::Rng;
use crate::state::{DocRec, OrgRec, PersonRec, ProjectRec, State, ThreadRec, TopicRec};
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How the seeder makes what it makes.
pub struct SeedOptions {
    /// The machine the threads run on (name or id); the first online one that plays the agent, environment and
    /// provider roles when not given.
    pub machine: Option<String>,
    /// The environment whose directory holds the orgs' environments' directories; one running on the machine when
    /// not given.
    pub carrier: Option<String>,
    pub model_protocol: String,
    pub model_upstream: String,
    pub model_key: Option<String>,
    pub model: String,
    pub sandbox_mode: Option<String>,
    /// How a thread's agent is placed (`together`, `separated`, …); the system's first permitted when not given.
    pub arrangement: Option<String>,
    pub concurrency: usize,
    pub orgs_per_person: usize,
    pub password: String,
}

/// Words the documents and the turns are written with, and that the searches look for.
pub const WORDS: [&str; 24] = [
    "orbit", "ledger", "harbor", "quartz", "meadow", "signal", "lantern", "cobalt", "summit",
    "willow", "ember", "prism", "canyon", "falcon", "glacier", "juniper", "nebula", "oracle",
    "pioneer", "rapids", "saffron", "tundra", "vertex", "zephyr",
];

fn say(text: &str) {
    eprintln!("{} {text}", chrono::Utc::now().format("%H:%M:%S"));
}

/// Runs `make` for every item, `n` at a time, and returns what each made (in the order they finished).
async fn many<T, F, Fut, R>(items: Vec<T>, n: usize, make: F) -> Vec<R>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R>,
{
    futures::stream::iter(items.into_iter().map(make))
        .buffer_unordered(n.max(1))
        .collect()
        .await
}

fn markdown(title: &str, seed: usize) -> String {
    let mut rng = Rng::new(seed as u64 + 17);
    let mut out = format!("# {title}\n\n");
    for p in 0..6 {
        let words: Vec<&str> = (0..40).map(|_| WORDS[rng.below(WORDS.len())]).collect();
        out.push_str(&format!(
            "Part {p} of note {seed}: {}.\n\n",
            words.join(" ")
        ));
    }
    out
}

fn paragraph(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "paragraph", "content": [{ "type": "text", "text": text }] })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Lets every member of the org use a resource the administrator made there, so that the people make and work in
/// their own threads: its page names the org's members with "see it and take part" (an agent profile, a provider),
/// or "see it, take part and manage" (an environment: binding a thread to one asks `environment.manage`).
async fn share_with_members(
    api: &Api,
    admin: &Person,
    object: &str,
    id: &str,
    org: &str,
) -> std::result::Result<(), String> {
    let choice = if object == "environments" {
        "full_manage"
    } else {
        "full_interact"
    };
    let change =
        json!({ "name": [{ "holders": [{ "kind": "members", "id": org }], "choice": choice }] });
    let reply = api
        .call(
            admin,
            reqwest::Method::PATCH,
            "/{object}/{id}/permissions",
            &[object, id],
            "",
            Some(&change),
        )
        .await;
    if reply.ok() {
        Ok(())
    } else {
        Err(format!(
            "{object} {id} could not be shared with the members of {org}: {}",
            reply.why()
        ))
    }
}

/// Seeds up to the figures. `state` is saved to `path` after each kind of thing.
pub async fn seed(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    path: &Path,
    figures: &Figures,
    o: &SeedOptions,
    prefix: &str,
) -> Result<()> {
    let started = Instant::now();
    state.prefix = prefix.to_owned();
    // A build where an administrator's session starts without "As admin" needs it for /admin/... (making people);
    // a build without the switch has no such route, and it is not asked.
    let mode = api
        .put(admin, "/me/admin-mode", &[], &json!({ "as_admin": true }))
        .await;
    if mode.status != 0 && !mode.ok() {
        api.rec.note(format!(
            "{} could not act as administrator: {}",
            admin.login,
            mode.why()
        ));
    }
    find_machine(api, admin, state, o).await?;
    state.save(path)?;
    let people = make_people(api, admin, state, figures, o, prefix).await?;
    state.save(path)?;
    make_orgs(api, admin, state, figures, o, prefix).await?;
    state.save(path)?;
    make_memberships(api, admin, state, &people, o).await;
    state.save(path)?;
    make_environments(api, admin, state, o, prefix).await;
    state.save(path)?;
    make_providers(api, admin, state, figures, o).await;
    state.save(path)?;
    make_agents(api, admin, state, figures).await;
    state.save(path)?;
    make_projects(api, admin, state, figures, o).await;
    state.save(path)?;
    make_topics(api, admin, state, o).await;
    state.save(path)?;
    make_threads(api, admin, state, &people, figures, o).await;
    state.save(path)?;
    make_docs(api, admin, state, &people, figures, o).await;
    state.save(path)?;
    look_at_hosts(api, admin, state, figures).await;
    state.save(path)?;
    say(&format!(
        "seeded in {:.0} s: {} people, {} orgs, {} projects, {} topics, {} threads, {} documents, {} providers, {} agent profiles, {} hosts present",
        started.elapsed().as_secs_f64(),
        state.people.len(),
        state.orgs.len(),
        state.projects.len(),
        state.topics.len(),
        state.threads.len(),
        state.docs.len(),
        state.orgs.iter().map(|o| o.providers.len()).sum::<usize>(),
        state.orgs.iter().map(|o| o.agents.len()).sum::<usize>(),
        state.hosts.len(),
    ));
    Ok(())
}

fn has_roles(machine: &Value, roles: &[&str]) -> bool {
    let have: Vec<&str> = machine
        .get("roles")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    roles.iter().all(|r| have.contains(r))
}

/// The machine the threads run on: online, with the agent, environment and provider roles.
async fn find_machine(api: &Api, admin: &Person, state: &mut State, o: &SeedOptions) -> Result<()> {
    let reply = api.get(admin, "/machines", &[], "").await;
    if !reply.ok() {
        bail!("the machines could not be listed: {}", reply.why());
    }
    let machines = reply.body.as_array().cloned().unwrap_or_default();
    let chosen = machines.iter().find(|m| {
        let named = o.machine.as_deref().is_none_or(|want| {
            m.get("name").and_then(Value::as_str) == Some(want)
                || m.get("id").and_then(Value::as_str) == Some(want)
        });
        named
            && m.get("online").and_then(Value::as_bool) == Some(true)
            && has_roles(m, &["agent", "environment", "provider"])
    });
    let Some(machine) = chosen else {
        bail!(
            "no online machine of {} plays the agent, environment and provider roles{}: threads cannot run. Enroll a \
             connector with those roles and the fake harness listed in its agent.harnesses policy, or name one with --machine",
            admin.login,
            o.machine
                .as_deref()
                .map(|m| format!(" (asked for {m})"))
                .unwrap_or_default()
        );
    };
    let id = machine
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let name = machine
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    say(&format!("threads run on the machine {name} ({id})"));
    state.machine = Some((id, name));
    Ok(())
}

/// The people the drive acts as: made by the administrator, each choosing their password through the link the
/// administrator was given, as a person does.
async fn make_people(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    figures: &Figures,
    o: &SeedOptions,
    prefix: &str,
) -> Result<Vec<Arc<Person>>> {
    let have = state.people.len();
    let wanted: Vec<usize> = (have..figures.people).collect();
    if !wanted.is_empty() {
        say(&format!("making {} people", wanted.len()));
    }
    let made = many(wanted, o.concurrency, |i| async move {
        let login = format!("{prefix}-p{i:03}");
        let body = json!({ "username": login, "display_name": format!("Stress person {i:03}"), "email": format!("{login}@stress.test") });
        let reply = api.post(admin, "/admin/users", &[], &body).await;
        if reply.ok() {
            let url = reply.text("/reset_url").unwrap_or_default();
            let token = url.rsplit('/').next().unwrap_or_default().to_owned();
            let confirm = json!({ "token": token, "password": o.password });
            let set = api.anonymous_post("/auth/recovery/confirm", &confirm).await;
            if !set.ok() {
                return Err(format!("{login} could not choose a password: {}", set.why()));
            }
        }
        // Made now, or by an earlier run whose state was lost: either way they sign in with the password.
        match api.sign_in(&login, &o.password).await {
            Ok(person) => Ok((i, person)),
            Err(e) => Err(format!("{login}: {e}")),
        }
    })
    .await;
    let mut made: Vec<(usize, Arc<Person>)> = made
        .into_iter()
        .filter_map(|r| {
            r.map_err(|e| api.rec.note(format!("a person was not made: {e}")))
                .ok()
        })
        .collect();
    made.sort_by_key(|(i, _)| *i);
    for (_, person) in &made {
        state.people.push(PersonRec {
            login: person.login.clone(),
            id: person.id.clone(),
            orgs: Vec::new(),
        });
    }
    // Everybody signs in, for the memberships and the things made in their name.
    let logins: Vec<String> = state.people.iter().map(|p| p.login.clone()).collect();
    let signed = many(logins, o.concurrency, |login| async move {
        api.sign_in(&login, &o.password).await
    })
    .await;
    let mut people: Vec<Arc<Person>> = Vec::new();
    for person in signed {
        match person {
            Ok(person) => people.push(person),
            Err(e) => api.rec.note(format!("a person could not sign in: {e}")),
        }
    }
    people.sort_by_key(|p| {
        state
            .people
            .iter()
            .position(|r| r.login == p.login)
            .unwrap_or(usize::MAX)
    });
    if people.len() != state.people.len() {
        bail!(
            "only {} of {} people could sign in",
            people.len(),
            state.people.len()
        );
    }
    Ok(people)
}

async fn make_orgs(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    figures: &Figures,
    o: &SeedOptions,
    prefix: &str,
) -> Result<()> {
    let have = state.orgs.len();
    let wanted: Vec<usize> = (have..figures.orgs).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    say(&format!("making {} orgs", wanted.len()));
    let http_upstream = o.model_upstream.starts_with("http://");
    let made = many(wanted, o.concurrency, |i| async move {
        let slug = format!("{prefix}-org-{i:03}");
        let body = json!({ "name": format!("Stress org {i:03}"), "slug": slug });
        let reply = api.post(admin, "/orgs", &[], &body).await;
        let id = if reply.ok() {
            reply.id()
        } else {
            // Made by an earlier run whose state was lost.
            api.get(admin, "/orgs/by-slug/{slug}", &[&slug], "")
                .await
                .id()
        };
        let Some(id) = id else {
            return Err(format!("{slug}: {}", reply.why()));
        };
        if let Some(mode) = &o.sandbox_mode {
            let set = api
                .put(
                    admin,
                    "/nodes/{id}/settings/{key}",
                    &[&id, "sandbox.mode"],
                    &json!({ "value": mode }),
                )
                .await;
            if !set.ok() {
                return Err(format!(
                    "{slug}: sandbox.mode {mode} was refused: {}",
                    set.why()
                ));
            }
        }
        if http_upstream {
            let key = "providers.allow_http_upstreams";
            let set = api
                .put(
                    admin,
                    "/nodes/{id}/settings/{key}",
                    &[&id, key],
                    &json!({ "value": true }),
                )
                .await;
            if !set.ok() {
                return Err(format!("{slug}: {key} was refused: {}", set.why()));
            }
        }
        Ok((
            i,
            OrgRec {
                id,
                slug,
                environment: None,
                providers: Vec::new(),
                agents: Vec::new(),
                members: Vec::new(),
            },
        ))
    })
    .await;
    let mut made: Vec<(usize, OrgRec)> = made
        .into_iter()
        .filter_map(|r| {
            r.map_err(|e| api.rec.note(format!("an org was not made: {e}")))
                .ok()
        })
        .collect();
    made.sort_by_key(|(i, _)| *i);
    state.orgs.extend(made.into_iter().map(|(_, org)| org));
    Ok(())
}

/// Each person is a developer in `orgs_per_person` orgs, spread so that every org has members: invited by the
/// administrator, accepted by the person.
async fn make_memberships(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    people: &[Arc<Person>],
    o: &SeedOptions,
) {
    let n_orgs = state.orgs.len();
    if n_orgs == 0 {
        return;
    }
    let mut wanted = Vec::new();
    for p in 0..state.people.len() {
        for k in 0..o.orgs_per_person.min(n_orgs) {
            let org = (p + k) % n_orgs;
            if !state.people[p].orgs.contains(&org) {
                wanted.push((p, org));
            }
        }
    }
    if wanted.is_empty() {
        return;
    }
    say(&format!(
        "inviting people into orgs: {} memberships",
        wanted.len()
    ));
    let orgs = state.orgs.clone();
    let made = many(wanted, o.concurrency, |(p, org)| {
        let orgs = &orgs;
        async move {
            let body = json!({ "kind": "org", "org_id": orgs[org].id, "role": "developer", "max_uses": 1, "note": "orch-stress" });
            let invited = api.post(admin, "/invitations", &[], &body).await;
            let Some(token) = invited.text("/token") else {
                return Err(format!("{} into {}: {}", people[p].login, orgs[org].slug, invited.why()));
            };
            let accepted = api.post(&people[p], "/invitations/accept", &[], &json!({ "token": token })).await;
            if !accepted.ok() && accepted.status != 409 {
                return Err(format!("{} into {}: {}", people[p].login, orgs[org].slug, accepted.why()));
            }
            Ok((p, org))
        }
    })
    .await;
    for r in made {
        match r {
            Ok((p, org)) => {
                state.people[p].orgs.push(org);
                state.orgs[org].members.push(p);
            }
            Err(e) => api.rec.note(format!("a membership was not made: {e}")),
        }
    }
}

async fn wait_running(
    api: &Api,
    admin: &Person,
    environment: &str,
) -> std::result::Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let reply = api
            .get(admin, "/environments/{id}", &[environment], "")
            .await;
        match reply.text("/status").as_deref() {
            Some("running") => return Ok(()),
            Some("failed") => {
                return Err(reply
                    .text("/status_detail")
                    .unwrap_or_else(|| "failed".to_owned()));
            }
            _ if Instant::now() > deadline => {
                return Err(format!(
                    "not running after 90 s ({})",
                    reply.text("/status").unwrap_or_default()
                ));
            }
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

/// One environment per org on the machine, each in a directory of its own under the carrier's directory (made
/// through the carrier's file API: a request cannot make a directory on a machine any other way).
async fn make_environments(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    o: &SeedOptions,
    prefix: &str,
) {
    let Some((machine, _)) = state.machine.clone() else {
        return;
    };
    let wanted: Vec<usize> = (0..state.orgs.len())
        .filter(|i| state.orgs[*i].environment.is_none())
        .collect();
    if wanted.is_empty() {
        return;
    }
    let carrier = match find_carrier(api, admin, state, o, &machine).await {
        Ok(c) => c,
        Err(e) => {
            api.rec
                .note(format!("no environment could be made for the orgs: {e}"));
            return;
        }
    };
    let (carrier_id, carrier_root) = carrier;
    state.carrier = Some(carrier_id.clone());
    let base = format!("orch-{prefix}");
    let made_base = api
        .post(
            admin,
            "/environments/{id}/files/mkdir",
            &[&carrier_id],
            &json!({ "path": base }),
        )
        .await;
    // A machine may accept no file written from outside (its `environment.remote_write`): then every org's
    // environment has the carrier's own directory, which exists. The scripted harness writes nothing in it.
    let shared = !made_base.ok() && made_base.status != 409;
    if shared {
        api.rec.note(format!(
            "the machine makes no directory asked from outside ({}): the orgs' environments share the carrier's directory {carrier_root}",
            made_base.why().trim()
        ));
    } else {
        say(&format!(
            "making {} environments under {carrier_root}/{base}",
            wanted.len()
        ));
    }
    let orgs = state.orgs.clone();
    let made = many(wanted, o.concurrency, |i| {
        let (orgs, carrier_id, carrier_root, base, machine) = (&orgs, &carrier_id, &carrier_root, &base, &machine);
        async move {
            let slug = &orgs[i].slug;
            let root = if shared {
                carrier_root.clone()
            } else {
                let dir = format!("{base}/{slug}");
                let mkdir = api.post(admin, "/environments/{id}/files/mkdir", &[carrier_id], &json!({ "path": dir })).await;
                if !mkdir.ok() && mkdir.status != 409 {
                    return Err(format!("{slug}: the directory {dir}: {}", mkdir.why()));
                }
                format!("{}/{dir}", carrier_root.trim_end_matches('/'))
            };
            let body = json!({ "home_id": orgs[i].id, "name": format!("Stress workspace {slug}"), "root": root });
            let reply = api.post(admin, "/machines/{id}/environments", &[machine], &body).await;
            let Some(id) = reply.id() else {
                return Err(format!("{slug}: {}", reply.why()));
            };
            wait_running(api, admin, &id).await.map_err(|e| format!("{slug}: environment {id}: {e}"))?;
            if let Err(e) = share_with_members(api, admin, "environments", &id, &orgs[i].id).await {
                api.rec.note(e);
            }
            Ok((i, id))
        }
    })
    .await;
    for r in made {
        match r {
            Ok((i, id)) => state.orgs[i].environment = Some(id),
            Err(e) => api.rec.note(format!("an environment was not made: {e}")),
        }
    }
}

/// The carrier: given, or the first running environment on the machine that the administrator sees.
async fn find_carrier(
    api: &Api,
    admin: &Person,
    state: &State,
    o: &SeedOptions,
    machine: &str,
) -> std::result::Result<(String, String), String> {
    let wanted = o.carrier.clone().or_else(|| state.carrier.clone());
    if let Some(id) = wanted {
        let reply = api.get(admin, "/environments/{id}", &[&id], "").await;
        let root = reply
            .text("/workspace_root")
            .ok_or_else(|| format!("the carrier {id}: {}", reply.why()))?;
        return Ok((id, root));
    }
    let reply = api.get(admin, "/environments", &[], "").await;
    let list = reply.body.as_array().cloned().unwrap_or_default();
    for env in &list {
        let on_machine = env.pointer("/machine/id").and_then(Value::as_str) == Some(machine);
        let running = env.get("status").and_then(Value::as_str) == Some("running");
        let id = env.get("id").and_then(Value::as_str).unwrap_or_default();
        if !(on_machine && running) || id.is_empty() {
            continue;
        }
        let one = api.get(admin, "/environments/{id}", &[id], "").await;
        if let Some(root) = one.text("/workspace_root") {
            say(&format!(
                "the carrier is the environment {} ({id}) at {root}",
                env.get("name").and_then(Value::as_str).unwrap_or("?")
            ));
            return Ok((id.to_owned(), root));
        }
    }
    Err(format!(
        "no running environment on the machine to make directories in ({} listed): name one with --carrier",
        list.len()
    ))
}

/// API providers on the machine, spread over the orgs, each with the model the threads use.
async fn make_providers(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    figures: &Figures,
    o: &SeedOptions,
) {
    let Some((machine, _)) = state.machine.clone() else {
        return;
    };
    let have: usize = state.orgs.iter().map(|org| org.providers.len()).sum();
    let n_orgs = state.orgs.len();
    if n_orgs == 0 || have >= figures.providers {
        return;
    }
    let wanted: Vec<usize> = (have..figures.providers).collect();
    say(&format!("making {} providers", wanted.len()));
    let orgs = state.orgs.clone();
    let made = many(wanted, o.concurrency, |j| {
        let (orgs, machine) = (&orgs, &machine);
        async move {
            let org = j % n_orgs;
            let body = json!({
                "kind": "api", "name": format!("Stress model {j:03}"), "home_id": orgs[org].id,
                "upstreams": [{ "protocol": o.model_protocol, "base_url": o.model_upstream, "options": {} }],
            });
            let reply = api.post(admin, "/machines/{id}/providers", &[machine], &body).await;
            let Some(id) = reply.id() else {
                return Err(format!("provider {j}: {}", reply.why()));
            };
            if let Some(key) = &o.model_key {
                let set = api.put(admin, "/providers/{id}/key", &[&id], &json!({ "api_key": key })).await;
                if !set.ok() {
                    return Err(format!("provider {j}: its key: {}", set.why()));
                }
            }
            let model = json!({ "model_name": o.model, "display_name": o.model });
            let added = api.post(admin, "/providers/{id}/models", &[&id], &model).await;
            if !added.ok() && added.status != 409 {
                return Err(format!("provider {j}: the model {}: {}", o.model, added.why()));
            }
            // A provider is looked at again when asked: its status reads ready only after that.
            let refreshed = api.post(admin, "/providers/{id}/refresh", &[&id], &json!({})).await;
            let status = refreshed.text("/status").unwrap_or_else(|| refreshed.why());
            if status != "ready" {
                api.rec.note(format!("provider {id} of org {} is {status} after a refresh", orgs[org].slug));
            }
            if let Err(e) = share_with_members(api, admin, "providers", &id, &orgs[org].id).await {
                api.rec.note(e);
            }
            Ok((org, id))
        }
    })
    .await;
    for r in made {
        match r {
            Ok((org, id)) => state.orgs[org].providers.push(id),
            Err(e) => api.rec.note(format!("a provider was not made: {e}")),
        }
    }
}

/// Agent profiles of the scripted harness (`fake`), spread over the orgs.
async fn make_agents(api: &Api, admin: &Arc<Person>, state: &mut State, figures: &Figures) {
    let have: usize = state.orgs.iter().map(|org| org.agents.len()).sum();
    let n_orgs = state.orgs.len();
    if n_orgs == 0 || have >= figures.agents {
        return;
    }
    say(&format!("making {} agent profiles", figures.agents - have));
    for j in have..figures.agents {
        let org = j % n_orgs;
        let body = json!({ "name": format!("Stress agent {j:03}"), "harness_kind": "fake", "profile": {} });
        let reply = api
            .post(admin, "/orgs/{id}/agents", &[&state.orgs[org].id], &body)
            .await;
        match reply.id() {
            Some(id) => {
                if let Err(e) =
                    share_with_members(api, admin, "agents", &id, &state.orgs[org].id).await
                {
                    api.rec.note(e);
                }
                state.orgs[org].agents.push(id);
            }
            None => api
                .rec
                .note(format!("agent profile {j} was not made: {}", reply.why())),
        }
    }
}

async fn make_projects(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    figures: &Figures,
    o: &SeedOptions,
) {
    let n_orgs = state.orgs.len();
    let have = state.projects.len();
    if n_orgs == 0 || have >= figures.projects {
        return;
    }
    say(&format!("making {} projects", figures.projects - have));
    let orgs = state.orgs.clone();
    let made = many((have..figures.projects).collect(), o.concurrency, |j| {
        let orgs = &orgs;
        async move {
            let org = j % n_orgs;
            let name = format!("Project {j:03}");
            let reply = api
                .post(
                    admin,
                    "/orgs/{id}/projects",
                    &[&orgs[org].id],
                    &json!({ "name": name }),
                )
                .await;
            reply
                .id()
                .map(|id| (j, ProjectRec { id, name, org }))
                .ok_or_else(|| format!("project {j}: {}", reply.why()))
        }
    })
    .await;
    let mut ok: Vec<(usize, ProjectRec)> = made
        .into_iter()
        .filter_map(|r| {
            r.map_err(|e| api.rec.note(format!("a project was not made: {e}")))
                .ok()
        })
        .collect();
    ok.sort_by_key(|(j, _)| *j);
    state.projects.extend(ok.into_iter().map(|(_, p)| p));
}

/// One topic in each project.
async fn make_topics(api: &Api, admin: &Arc<Person>, state: &mut State, o: &SeedOptions) {
    let wanted: Vec<usize> = (0..state.projects.len())
        .filter(|p| !state.topics.iter().any(|t| t.project == *p))
        .collect();
    if wanted.is_empty() {
        return;
    }
    say(&format!("making {} topics", wanted.len()));
    let projects = state.projects.clone();
    let made = many(wanted, o.concurrency, |p| {
        let projects = &projects;
        async move {
            let name = format!("Topic {p:03}");
            let reply = api
                .post(
                    admin,
                    "/projects/{id}/topics",
                    &[&projects[p].id],
                    &json!({ "name": name }),
                )
                .await;
            reply
                .id()
                .map(|id| {
                    (
                        p,
                        TopicRec {
                            id,
                            name,
                            project: p,
                            org: projects[p].org,
                        },
                    )
                })
                .ok_or_else(|| format!("topic of project {p}: {}", reply.why()))
        }
    })
    .await;
    let mut ok: Vec<(usize, TopicRec)> = made
        .into_iter()
        .filter_map(|r| {
            r.map_err(|e| api.rec.note(format!("a topic was not made: {e}")))
                .ok()
        })
        .collect();
    ok.sort_by_key(|(p, _)| *p);
    state.topics.extend(ok.into_iter().map(|(_, t)| t));
}

/// Threads spread over the topics, each bound to its org's agent profile, provider and environment, made by a
/// member of the org who then works in it (by the administrator where the org has no member or a member may not).
async fn make_threads(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    people: &[Arc<Person>],
    figures: &Figures,
    o: &SeedOptions,
) {
    let have = state.threads.len();
    let n_topics = state.topics.len();
    if n_topics == 0 || have >= figures.threads {
        return;
    }
    say(&format!("making {} threads", figures.threads - have));
    let (orgs, topics) = (state.orgs.clone(), state.topics.clone());
    let made = many((have..figures.threads).collect(), o.concurrency, |j| {
        let (orgs, topics) = (&orgs, &topics);
        async move {
            let topic = j % n_topics;
            let org = &orgs[topics[topic].org];
            let (Some(environment), Some(agent), Some(provider)) = (
                org.environment.as_ref(),
                org.agents.get(j % org.agents.len().max(1)),
                org.providers.get(j % org.providers.len().max(1)),
            ) else {
                return Err(format!(
                    "thread {j}: the org {} lacks {}",
                    org.slug,
                    [("an environment", org.environment.is_none()), ("an agent profile", org.agents.is_empty()), ("a provider", org.providers.is_empty())]
                        .iter()
                        .filter(|(_, missing)| *missing)
                        .map(|(what, _)| *what)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            let name = format!("Thread {j:03}");
            let mut binding = json!({ "agent_id": agent, "provider_id": provider });
            if let Some(arrangement) = &o.arrangement {
                binding["arrangement"] = json!(arrangement);
            }
            let body = json!({ "name": name, "binding": binding, "model": o.model, "environment": { "use": environment } });
            let owner = (!org.members.is_empty()).then(|| org.members[j % org.members.len()]);
            let mut reply = match owner {
                Some(p) => api.post(&people[p], "/topics/{id}/threads", &[&topics[topic].id], &body).await,
                None => api.post(admin, "/topics/{id}/threads", &[&topics[topic].id], &body).await,
            };
            let mut made_by = owner;
            if !reply.ok() && owner.is_some() {
                let refused = reply.why();
                reply = api.post(admin, "/topics/{id}/threads", &[&topics[topic].id], &body).await;
                made_by = None;
                if reply.ok() {
                    api.rec.note(format!("thread {j}: its member could not make it ({refused}); the administrator did"));
                }
            }
            reply
                .id()
                .map(|id| (j, ThreadRec { id, name, topic, org: topics[topic].org, owner: made_by, provider: Some(provider.clone()) }))
                .ok_or_else(|| format!("thread {j}: {}", reply.why()))
        }
    })
    .await;
    let mut ok: Vec<(usize, ThreadRec)> = made
        .into_iter()
        .filter_map(|r| {
            r.map_err(|e| api.rec.note(format!("a thread was not made: {e}")))
                .ok()
        })
        .collect();
    ok.sort_by_key(|(j, _)| *j);
    state.threads.extend(ok.into_iter().map(|(_, t)| t));
}

/// Documents spread over the projects, written by members; every fifth rewritten twice (two versions), every fourth
/// discussed (a discussion with two answers).
async fn make_docs(
    api: &Api,
    admin: &Arc<Person>,
    state: &mut State,
    people: &[Arc<Person>],
    figures: &Figures,
    o: &SeedOptions,
) {
    let have = state.docs.len();
    let n_projects = state.projects.len();
    if n_projects == 0 || have >= figures.docs {
        return;
    }
    say(&format!("making {} documents", figures.docs - have));
    let (orgs, projects) = (state.orgs.clone(), state.projects.clone());
    let made = many((have..figures.docs).collect(), o.concurrency, |j| {
        let (orgs, projects) = (&orgs, &projects);
        async move {
            let project = j % n_projects;
            let org = &orgs[projects[project].org];
            let writer: &Person = if org.members.is_empty() { admin } else { &people[org.members[j % org.members.len()]] };
            let title = format!("Design note {j:04} {}", WORDS[j % WORDS.len()]);
            let body = json!({ "title": title, "markdown": markdown(&title, j) });
            let reply = api.post(writer, "/nodes/{id}/documents", &[&projects[project].id], &body).await;
            let Some(id) = reply.id() else {
                return Err(format!("document {j}: {}", reply.why()));
            };
            let mut doc = DocRec { id: id.clone(), title, project, org: projects[project].org, versions: 0, discussions: 0 };
            if j % 5 == 0 {
                for round in 0..2 {
                    let sync = api.get(writer, "/documents/{id}/sync", &[&id], "").await;
                    let live: Vec<String> = sync
                        .body
                        .get("blocks")
                        .and_then(Value::as_array)
                        .map(|b| {
                            b.iter()
                                .filter(|row| row.get("removed").and_then(Value::as_bool) != Some(true))
                                .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut ops: Vec<Value> = live.iter().map(|b| json!({ "op": "remove", "id": b })).collect();
                    let text = format!("Rewritten ({round}) note {j}: {}", WORDS[(j + round) % WORDS.len()]);
                    ops.push(json!({ "op": "insert", "block": paragraph(&new_id(), &text), "parent_id": null, "after_id": null }));
                    let rewrite = api.post(writer, "/documents/{id}/ops", &[&id], &json!({ "client_id": new_id(), "ops": ops })).await;
                    if rewrite.ok() {
                        doc.versions += 1;
                    } else {
                        return Err(format!("document {j}: rewrite {round}: {}", rewrite.why()));
                    }
                }
            }
            if j % 4 == 0 {
                let sync = api.get(writer, "/documents/{id}/sync", &[&id], "").await;
                let block = sync
                    .body
                    .pointer("/blocks/0/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_default();
                let text = |t: &str| json!([{ "type": "text", "text": t }]);
                let opened = api
                    .post(writer, "/documents/{id}/discussions", &[&id], &json!({ "block_id": block, "content": text("Is this still right?"), "client_id": new_id() }))
                    .await;
                let Some(discussion) = opened.id() else {
                    return Err(format!("document {j}: its discussion: {}", opened.why()));
                };
                doc.discussions += 1;
                for (k, answer) in ["Yes, for now.", "Let us check it next week."].iter().enumerate() {
                    let who: &Person = if org.members.len() > 1 { &people[org.members[(j + k + 1) % org.members.len()]] } else { writer };
                    let reply = api
                        .post(who, "/documents/{id}/discussions/{discussion_id}/comments", &[&id, &discussion], &json!({ "content": text(answer), "client_id": new_id() }))
                        .await;
                    if !reply.ok() {
                        return Err(format!("document {j}: an answer in its discussion: {}", reply.why()));
                    }
                }
            }
            Ok((j, doc))
        }
    })
    .await;
    let mut ok: Vec<(usize, DocRec)> = Vec::new();
    let mut failed: BTreeMap<String, usize> = BTreeMap::new();
    for r in made {
        match r {
            Ok(x) => ok.push(x),
            Err(e) => {
                let short: String = e.chars().take(200).collect();
                *failed.entry(short).or_default() += 1;
            }
        }
    }
    for (e, n) in failed {
        api.rec.note(format!("{n} document(s) not made whole: {e}"));
    }
    ok.sort_by_key(|(j, _)| *j);
    state.docs.extend(ok.into_iter().map(|(_, d)| d));
}

/// Hosts are machines whose node enrolled: listed and counted, never made.
async fn look_at_hosts(api: &Api, admin: &Arc<Person>, state: &mut State, figures: &Figures) {
    let reply = api.get(admin, "/hosts", &[], &query(&[])).await;
    let names: Vec<String> = reply
        .body
        .as_array()
        .map(|hosts| {
            hosts
                .iter()
                .filter_map(|h| h.get("name").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if names.len() < figures.hosts {
        api.rec.note(format!(
            "hosts: {} of the {} asked for exist ({}). A host is a machine whose node enrolled (`orch-node enroll` with a \
             token from POST /hosts/enrollment-tokens and a running node); the HTTP API alone cannot make one",
            names.len(),
            figures.hosts,
            names.join(", ")
        ));
    }
    state.hosts = names;
}

/// The administrator, signed in.
pub async fn admin(api: &Api, login: &str, password: &str) -> Result<Arc<Person>> {
    api.sign_in(login, password)
        .await
        .context("the administrator signs in")
}
