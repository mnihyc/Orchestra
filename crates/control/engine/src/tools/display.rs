//! The display tool opens or announces local screens and closes only the calling thread's own.

use super::definitions::{Code, Refusal};
use super::{Answer, CallContext};
use crate::displays::{
    CLOSED_LIST_KEPT, DisplayFailure, Displays, NOTE_MAX, OpenDisplay, Requester,
};
use crate::{engine::Engine, error::EngineError};
use orch_db::queries::{displays as rows, threads::ThreadRow};
use orch_types::{Id, Json};
use serde::Deserialize;
use serde_json::json;

/// The shipped description of the display tool.
pub const DESCRIPTION: &str = "Get a graphical display that the people on this work can watch and work in with you, or tell them of one you started. Use it for a program that has to show a window: a desktop application, an emulator, a browser with a window. list: the displays that are served and where. open: a virtual screen of the size you give as geometry (for example \"1280x800\"); the answer has its X display, which you set as DISPLAY for the programs you start on it, and its endpoint. announce: you started something that serves VNC yourself, on 127.0.0.1 or on a socket; give its endpoint, and its password if it has one, so that people see it. close: one you opened or announced. A display shows whatever is on it to everybody who may watch this environment's displays: keep secrets off it.";

/// The closed argument schema shared with every adapter.
pub fn schema() -> Json {
    json!({"type":"object","additionalProperties":false,"required":["command"],"properties":{
        "command":{"type":"string","enum":["list","open","announce","close"],"description":"Choose list to read the displays, open for a new virtual screen, announce for your own local VNC server, or close for a display this thread opened or announced."},
        "name":{"type":"string","description":"Listed name for a new virtual screen."},
        "geometry":{"type":"string","description":"Virtual screen dimensions, for example 1280x800."},
        "endpoint":{"type":"string","description":"Loopback VNC port or local socket path to announce."},
        "password":{"type":"string","description":"The announced server's password, kept sealed and given to no viewer."},
        "display":{"type":"string","description":"Identifier of a display you opened or announced, to close."},
        "note":{"type":"string","maxLength":NOTE_MAX,"description":"A short purpose line for people."}
    }})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    name: Option<String>,
    geometry: Option<String>,
    endpoint: Option<String>,
    password: Option<String>,
    display: Option<Id>,
    note: Option<String>,
}
impl Arguments {
    fn validate(&self) -> Result<(), Refusal> {
        let forbidden = match self.command.as_str() {
            "list" => {
                self.name.is_some()
                    || self.geometry.is_some()
                    || self.endpoint.is_some()
                    || self.password.is_some()
                    || self.display.is_some()
                    || self.note.is_some()
            }
            "open" => self.endpoint.is_some() || self.password.is_some() || self.display.is_some(),
            "announce" => {
                self.endpoint.is_none()
                    || self.name.is_some()
                    || self.geometry.is_some()
                    || self.display.is_some()
            }
            "close" => {
                self.display.is_none()
                    || self.name.is_some()
                    || self.geometry.is_some()
                    || self.endpoint.is_some()
                    || self.password.is_some()
                    || self.note.is_some()
            }
            _ => true,
        };
        if forbidden {
            return Err(Refusal::invalid(
                "list takes no other arguments; open takes name, geometry and note; announce needs endpoint and takes password and note; close needs display.",
            ));
        }
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.chars().count() > NOTE_MAX)
        {
            return Err(Refusal::invalid("note must be at most 200 characters"));
        }
        Ok(())
    }
}

impl Engine {
    /// The four display commands use the calling thread's environment without impersonating a person.
    pub(crate) async fn use_display_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let args: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(args) => args,
            Err(_) => {
                return Ok(Refusal::invalid(
                    "use_display needs a known command and its documented arguments",
                )
                .into());
            }
        };
        if let Err(refusal) = args.validate() {
            return Ok(refusal.into());
        }
        let Some(environment) = thread.environment_id else {
            return Ok(Refusal::new(Code::Disabled, "This thread has no environment.").into());
        };
        let mut conn = self.inner.db.acquire().await?;
        let Some(site) = rows::site(&mut conn, environment).await? else {
            return Ok(Refusal::new(Code::Disabled, "The environment no longer exists.").into());
        };
        let off = Displays::off(&mut conn, &site).await?;
        if off.is_some() && args.command != "list" {
            return Ok(Refusal::new(Code::Disabled,"Displays are off here. A person who manages the environment or its home can switch them on.").into());
        }
        drop(conn);
        let displays = self.displays();
        let result = match args.command.as_str() {
            "list" => {
                if off.is_none() {
                    let _ = self.browsers().look(environment).await;
                }
                let mut conn = self.inner.db.acquire().await?;
                let found = if off.is_none() {
                    rows::list(
                        &mut conn,
                        environment,
                        self.inner.clock.now() - CLOSED_LIST_KEPT,
                    )
                    .await?
                } else {
                    Vec::new()
                };
                let mut value = json!({"on":off.is_none(),"displays":found.into_iter().filter(|d|d.state!="closed").map(|d|{
                    let mut row=json!({"display":d.id,"name":d.name,"endpoint":d.endpoint,"opened_by":d.opened_by.or(d.thread_id),"mine":d.opened_by.is_none()&&d.thread_id==Some(thread.node_id)});
                    if let Some(x)=d.x_display{row["x_display"]=json!(x);}
                    if let Some(g)=d.geometry{row["geometry"]=json!(g);}
                    if let Some(n)=d.note{row["note"]=json!(n);}
                    row
                }).collect::<Vec<_>>(),"servers":site.facts["vnc"]["servers"].as_array().into_iter().flatten().map(|p|json!({"name":p["name"],"path":p["path"]})).collect::<Vec<_>>()});
                if let Some(off) = off {
                    value["why_off"] = serde_json::to_value(off)?;
                }
                return Ok(Answer::Done(value));
            }
            "open" => {
                displays
                    .open(OpenDisplay {
                        environment,
                        requester: Requester::Agent(thread.node_id),
                        thread: Some(thread.node_id),
                        name: args.name,
                        geometry: args.geometry,
                        note: args.note,
                        origin: Default::default(),
                    })
                    .await
            }
            "announce" => {
                displays
                    .adopt(
                        environment,
                        thread.node_id,
                        args.endpoint.unwrap_or_default(),
                        args.password,
                        args.note,
                    )
                    .await
            }
            "close" => {
                let id = args.display.unwrap_or_default();
                let mut conn = self.inner.db.acquire().await?;
                let Some(row) = rows::get(&mut conn, id)
                    .await?
                    .filter(|r| r.environment_id == environment)
                else {
                    return Ok(Refusal::new(
                        Code::NotFound,
                        "No display with that id is known here; list shows those that are.",
                    )
                    .into());
                };
                if row.opened_by.is_some() || row.thread_id != Some(thread.node_id) {
                    return Ok(Refusal::new(
                        Code::NotYours,
                        "Close only a display that this thread opened or announced.",
                    )
                    .into());
                }
                drop(conn);
                return match displays
                    .close(
                        id,
                        "closed_by_agent",
                        Requester::Agent(thread.node_id),
                        &Default::default(),
                    )
                    .await
                {
                    Ok(_) => Ok(Answer::Done(json!({"closed":true}))),
                    Err(error) => refusal(error),
                };
            }
            _ => return Ok(Refusal::invalid("unknown display command").into()),
        };
        match result {
            Ok(row) => {
                let mut value = json!({"display":row.id,"name":row.name,"endpoint":row.endpoint,"watch":format!("the display {}, in {}",row.name,site.name)});
                if let Some(x) = row.x_display {
                    value["x_display"] = json!(x);
                }
                if let Some(g) = row.geometry {
                    value["geometry"] = json!(g);
                }
                Ok(Answer::Done(value))
            }
            Err(error) => refusal(error),
        }
    }
}
fn refusal(error: DisplayFailure) -> Result<Answer, EngineError> {
    let (code,message)=match error{
        DisplayFailure::NotFound=>(Code::NotFound,"The display is no longer known here.".to_owned()),
        DisplayFailure::Off(_)|DisplayFailure::DisplayOff(_)=>(Code::Disabled,"Displays are off here; a person can switch them on.".to_owned()),
        DisplayFailure::Unsupported=>(Code::Unsupported,"No virtual display server was found; announce takes a VNC server you start yourself.".to_owned()),
        DisplayFailure::TooMany{open,max}|DisplayFailure::DisplayTooMany{open,max}=>(Code::TooMany,format!("{open} owned displays run; the limit is {max}. You may close one of your own.")),
        DisplayFailure::Invalid(message)=>(Code::Invalid,message),
        DisplayFailure::Command(error)=>(if error.code=="not_local"{Code::NotLocal}else{Code::NotReachable},"The local VNC server could not be reached or authenticated. Check its endpoint and password.".to_owned()),
        DisplayFailure::Unreachable(_)=>(Code::NotReachable,"The environment did not answer.".to_owned()),
        DisplayFailure::Permission(_)=>(Code::Disabled,"The environment no longer belongs to this call.".to_owned()),
        DisplayFailure::Engine(error)=>return Err(error),
    };
    Ok(Refusal::new(code, message).into())
}

/// Whether a new session may offer the display tool, including an environment waiting to start.
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
    // As for browsers: advertised only where the machine that holds the environment permits it.
    Ok(
        !matches!(Displays::off(conn,&site).await?,Some(off) if off!=orch_api_contract::browsers::BrowsersOff::EnvironmentOffline)
            && orch_db::queries::environment_facts::permits(conn, environment, "environment.vnc")
                .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_password_belongs_only_to_announcement_and_never_to_a_list_or_open() {
        for command in ["list", "open", "close"] {
            let args: Arguments =
                serde_json::from_value(json!({"command":command,"password":"private"})).unwrap();
            assert!(args.validate().is_err());
        }
        let args: Arguments = serde_json::from_value(
            json!({"command":"announce","endpoint":"localhost:5900","password":"private"}),
        )
        .unwrap();
        assert!(args.validate().is_ok());
    }
}
