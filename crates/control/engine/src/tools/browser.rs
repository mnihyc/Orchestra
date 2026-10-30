//! The browser tool's four commands, checked against the calling thread's own environment.

use orch_api_contract::browsers::BrowserPreset;
use orch_db::queries::{browsers as rows, threads::ThreadRow};
use orch_link_contract::browser::BrowserProfile;
use orch_types::{Id, Json};
use serde::Deserialize;
use serde_json::json;

use super::definitions::{Code, Refusal};
use super::{Answer, CallContext};
use crate::browsers::{BrowserFailure, Browsers, CLOSED_LIST_KEPT, OpenBrowser, Requester};
use crate::engine::Engine;
use crate::error::EngineError;

/// A note names a browser in a sentence, with no page content.
const NOTE_CHARS: usize = super::browser_definition::NOTE_MAX;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    profile: Option<String>,
    preset: Option<BrowserPreset>,
    url: Option<String>,
    endpoint: Option<String>,
    browser: Option<Id>,
    note: Option<String>,
}

impl Arguments {
    fn validate(&self) -> Result<(), Refusal> {
        let forbidden = match self.command.as_str() {
            "list" => {
                self.profile.is_some()
                    || self.preset.is_some()
                    || self.url.is_some()
                    || self.endpoint.is_some()
                    || self.browser.is_some()
                    || self.note.is_some()
            }
            "open" => self.endpoint.is_some() || self.browser.is_some(),
            "announce" => {
                self.profile.is_some()
                    || self.preset.is_some()
                    || self.url.is_some()
                    || self.browser.is_some()
                    || self.endpoint.is_none()
            }
            "close" => {
                self.profile.is_some()
                    || self.preset.is_some()
                    || self.url.is_some()
                    || self.endpoint.is_some()
                    || self.note.is_some()
                    || self.browser.is_none()
            }
            _ => true,
        };
        if forbidden {
            return Err(Refusal::invalid(
                "list takes no other arguments; open takes profile, preset, url and note; announce needs endpoint and takes note; close needs browser.",
            ));
        }
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.chars().count() > NOTE_CHARS)
        {
            return Err(Refusal::invalid("note must be at most 200 characters"));
        }
        Ok(())
    }
}

impl Engine {
    /// Handles a browser call without asking a person's permission page for an agent's own processes.
    pub(crate) async fn use_browser_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let args: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(_) => {
                return Ok(Refusal::invalid(
                    "use_browser needs a known command and its documented arguments",
                )
                .into());
            }
        };
        if let Err(error) = args.validate() {
            return Ok(error.into());
        }
        let Some(environment) = thread.environment_id else {
            return Ok(Refusal::new(
                Code::Disabled,
                "This thread has no environment. A person can give it one.",
            )
            .into());
        };
        let browsers = self.browsers();
        let mut conn = self.inner.db.acquire().await?;
        let Some(site) = rows::site(&mut conn, environment).await? else {
            return Ok(Refusal::new(Code::Disabled, "The environment no longer exists.").into());
        };
        let off = Browsers::off(&mut conn, &site).await?;
        if let Some(off) = off
            && args.command != "list"
        {
            return Ok(Refusal::new(Code::Disabled,format!("Browsers are off here ({off:?}). A person who manages the environment or its home can switch them on. You can still run a browser yourself; nobody will see it.")).into());
        }
        drop(conn);
        match args.command.as_str() {
            "list" => {
                if off.is_none() {
                    let _ = browsers.look(environment).await;
                }
                let mut conn = self.inner.db.acquire().await?;
                let mut found = rows::list(
                    &mut conn,
                    environment,
                    self.inner.clock.now() - CLOSED_LIST_KEPT,
                )
                .await?;
                found.retain(|r| r.state != "closed");
                found.sort_by_key(|r| r.thread_id != Some(thread.node_id) || r.opened_by.is_some());
                for row in &found {
                    rows::touch(&mut conn, row.id, self.inner.clock.now()).await?;
                }
                let profiles: Vec<BrowserProfile> =
                    browsers.memory().profiles(environment).unwrap_or_default();
                let result = json!({"on":off.is_none(),"profiles_dir":profiles_dir(&site),"profiles":profiles.into_iter().map(|p|{
                    let mut value=json!({"name":p.name});if let Some(browser)=p.browser {value["in_use_by"]=json!(browser);}value
                }).collect::<Vec<_>>(),"browsers":found.into_iter().map(|b|{
                    let mut value=json!({"browser":b.id,"endpoint":b.endpoint,"opened_by":b.opened_by.or(b.thread_id),"mine":b.opened_by.is_none()&&b.thread_id==Some(thread.node_id)});
                    if let Some(profile)=b.profile {value["profile"]=json!(profile);}
                    if let Some(note)=b.note {value["note"]=json!(note);}
                    if let Some(tabs)=browsers.memory().tabs(b.id) {value["tabs"]=json!(tabs);}value
                }).collect::<Vec<_>>(),"programs":site.facts["browser"]["programs"],"presets":if site.facts["browser"]["window"]==true {vec!["headless","window"]}else{vec!["headless"]}});
                let mut result = result;
                if let Some(off) = off {
                    result["why_off"] = serde_json::to_value(off)?;
                    result["browsers"] = json!([]);
                    result["profiles"] = json!([]);
                }
                if let Some(cli) = site.facts["browser"]["playwright_cli"].as_str() {
                    result["playwright_cli"] = json!(cli);
                }
                Ok(Answer::Done(result))
            }
            "open" => {
                let profile = args.profile.unwrap_or_else(|| own_profile(thread.node_id));
                let opened = browsers
                    .open(OpenBrowser {
                        environment,
                        requester: Requester::Agent(thread.node_id),
                        thread: Some(thread.node_id),
                        profile,
                        create: true,
                        preset: args.preset,
                        url: args.url,
                        note: args.note,
                        origin: Default::default(),
                    })
                    .await;
                match opened {
                    Ok(opened) => Ok(Answer::Done(open_answer(
                        &opened.row,
                        &site,
                        opened.started,
                    ))),
                    Err(error) => {
                        self.browser_refusal(error, environment, thread.node_id)
                            .await
                    }
                }
            }
            "announce" => {
                let endpoint = args.endpoint.unwrap_or_default();
                match browsers
                    .adopt(environment, thread.node_id, endpoint, args.note)
                    .await
                {
                    Ok(row) => Ok(Answer::Done(open_answer(&row, &site, false))),
                    Err(error) => {
                        self.browser_refusal(error, environment, thread.node_id)
                            .await
                    }
                }
            }
            "close" => {
                let id = args.browser.unwrap_or_default();
                let mut conn = self.inner.db.acquire().await?;
                let Some(row) = rows::get(&mut conn, id).await? else {
                    return Ok(Refusal::new(
                        Code::NotFound,
                        "No browser with that id is known here; list shows those that are.",
                    )
                    .into());
                };
                if row.environment_id != environment {
                    return Ok(Refusal::new(
                        Code::NotFound,
                        "No browser with that id is known here; list shows those that are.",
                    )
                    .into());
                }
                if row.opened_by.is_some() || row.thread_id != Some(thread.node_id) {
                    return Ok(Refusal::new(Code::NotYours,format!("Browser {} was opened by {:?}. Share it as it is; close only one you opened or announced.",row.id,row.opened_by.or(row.thread_id))).into());
                }
                drop(conn);
                match browsers
                    .close(
                        id,
                        "closed_by_agent",
                        Requester::Agent(thread.node_id),
                        &Default::default(),
                    )
                    .await
                {
                    Ok(_) => Ok(Answer::Done(json!({"closed":true}))),
                    Err(error) => {
                        self.browser_refusal(error, environment, thread.node_id)
                            .await
                    }
                }
            }
            _ => Ok(Refusal::invalid("unknown browser command").into()),
        }
    }

    async fn browser_refusal(
        &self,
        error: BrowserFailure,
        environment: Id,
        thread: Id,
    ) -> Result<Answer, EngineError> {
        let (code,message)=match error {
            BrowserFailure::Off(off)=>(Code::Disabled,format!("Browsers are off here ({off:?}); a person can switch them on. You can still run one yourself, without a view in Orch.")),
            BrowserFailure::DisplayOff(_)=>(Code::Disabled,"Displays are off here. Use a headless browser or ask a person to switch displays on.".to_owned()),
            BrowserFailure::Unsupported=>(Code::Unsupported,"No Chromium-family browser was found. announce takes one you start with a program of your own and --remote-debugging-port=0.".to_owned()),
            BrowserFailure::DisplayTooMany{open,max}=>(Code::TooMany,format!("{open} displays run; the limit is {max}. Use a headless browser or close an owned display.")),
            BrowserFailure::TooMany{open,max}=>{
                let mut conn=self.inner.db.acquire().await?;let mut rows=rows::list(&mut conn,environment,self.inner.clock.now()-CLOSED_LIST_KEPT).await?;
                rows.retain(|r|r.state!="closed");rows.sort_by_key(|r|r.thread_id!=Some(thread)||r.opened_by.is_some());
                (Code::TooMany,format!("{open} browsers run; the limit is {max}. Running: {}. You may close one of your own.",rows.iter().map(|r|format!("{} on {}{}",r.id,r.profile.as_deref().unwrap_or("an unnamed profile"),if r.thread_id==Some(thread)&&r.opened_by.is_none(){" (yours)"}else{""})).collect::<Vec<_>>().join(", ")))
            }
            BrowserFailure::NotFound=>(Code::NotFound,"No browser with that id is known here; list shows those that are.".to_owned()),
            BrowserFailure::Invalid(message)=>(Code::Invalid,message),
            BrowserFailure::Command(error)=>(match error.code.as_str(){"not_local"=>Code::NotLocal,"not_a_browser"|"not_reachable"=>Code::NotReachable,"unsupported"=>Code::Unsupported,"not_found"=>Code::NotFound,_=>Code::Invalid},format!("{}; a browser you announce needs --remote-debugging-port=0, and its endpoint must be a loopback port here.",error.detail)),
            BrowserFailure::Unreachable(message)=>(Code::NotReachable,message),
            BrowserFailure::Permission(_)=>(Code::Disabled,"The environment no longer belongs to this call.".to_owned()),
            BrowserFailure::Engine(error)=>return Err(error),
        };
        Ok(Refusal::new(code, message).into())
    }
}

/// A full thread identity keeps sign-ins separate even when UUID time prefixes agree.
fn own_profile(thread: Id) -> String {
    format!("thread-{}", thread.simple())
}

fn profiles_dir(site: &rows::Site) -> Option<String> {
    site.facts["browser_profiles_dir"]
        .as_str()
        .map(str::to_owned)
}

fn open_answer(row: &rows::BrowserRow, site: &rows::Site, started: bool) -> Json {
    let attach = if site.facts["browser"]["playwright_cli"].is_string() {
        format!("playwright-cli attach --cdp={}", row.endpoint)
    } else {
        format!(
            "{} — any tool for the browser's protocol connects here.",
            row.endpoint
        )
    };
    let mut value = json!({"browser":row.id,"endpoint":row.endpoint,"started":started,"attach":attach,"watch":format!("the browser on the profile {}, in {}",row.profile.as_deref().unwrap_or("an unnamed profile"),site.name)});
    if let Some(profile) = &row.profile {
        value["profile"] = json!(profile);
    }
    value
}

/// Whether the browser tool can be offered at the next session opening.
pub async fn offered(
    conn: &mut orch_db::Conn,
    environment: Option<Id>,
) -> Result<bool, EngineError> {
    let Some(environment) = environment else {
        return Ok(false);
    };
    let Some(site) = rows::site(conn, environment).await? else {
        return Ok(false);
    };
    // A door is advertised to agents only where the machine that holds the environment permits it: one whose
    // machine is not known is listed offline to people, and offered to no agent.
    Ok(
        !matches!(Browsers::off(conn,&site).await?,Some(off) if off!=orch_api_contract::browsers::BrowsersOff::EnvironmentOffline)
            && orch_db::queries::environment_facts::permits(
                conn,
                environment,
                "environment.browser",
            )
            .await?,
    )
}

/// The preamble names the programs actually found and the directory that keeps sign-ins.
pub async fn preamble(conn: &mut orch_db::Conn, environment: Id) -> Result<String, EngineError> {
    let Some(site) = rows::site(conn, environment).await? else {
        return Ok(String::new());
    };
    if !offered(conn, Some(environment)).await? {
        return Ok(String::new());
    }
    let profiles = profiles_dir(&site).unwrap_or_else(|| "ORCH_BROWSER_PROFILES".to_owned());
    if site.facts["browser"]["programs"]
        .as_array()
        .is_none_or(|p| p.is_empty())
    {
        return Ok("Browsers: no supported browser was found. use_browser announce takes one you start yourself with --remote-debugging-port=0.".to_owned());
    }
    let driver = match site.facts["browser"]["playwright_cli"].as_str() {
        Some(cli) => format!(
            "Drive it with playwright-cli (installed, {cli}): the answer gives the command."
        ),
        None => {
            "The endpoint is an address any tool for the browser's protocol connects to.".to_owned()
        }
    };
    Ok(format!(
        "Browsers: use_browser opens one that people can watch. {driver} Profiles are kept in {profiles}. Share a running profile's browser and work in tabs you open yourself."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_with_equal_time_prefixes_have_distinct_complete_profile_names() {
        let a: Id = "019908ab-1111-7000-8000-000000000001".parse().unwrap();
        let b: Id = "019908ab-1111-7000-8000-000000000002".parse().unwrap();
        assert_eq!(own_profile(a), "thread-019908ab111170008000000000000001");
        assert_ne!(own_profile(a), own_profile(b));
        assert!(own_profile(a).len() <= 63);
    }

    #[test]
    fn open_and_announce_accept_a_whole_unicode_note_and_refuse_one_character_more() {
        for command in ["open", "announce"] {
            let mut value = json!({"command": command, "note": "界".repeat(NOTE_CHARS)});
            if command == "announce" {
                value["endpoint"] = json!("http://127.0.0.1:41821");
            }
            let arguments: Arguments = serde_json::from_value(value.clone()).unwrap();
            assert!(arguments.validate().is_ok());
            value["note"] = json!("界".repeat(NOTE_CHARS + 1));
            let arguments: Arguments = serde_json::from_value(value).unwrap();
            assert!(arguments.validate().is_err());
        }
    }
}
