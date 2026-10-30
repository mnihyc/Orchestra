//! The browser tool's closed schema and truthful effect hints.

use orch_types::Json;
use serde_json::json;

/// The shipped description of the browser tool.
pub const DESCRIPTION: &str = "Get a web browser that the people on this work can watch and drive with you, or tell them of one you started. Browsers run in your environment; a profile is a named directory there that keeps the sign-ins made in it, and a profile has one running browser at a time. list: the profiles, the browsers that run and where each is driven (its endpoint, an address on 127.0.0.1), and what is installed to drive one. open: a browser on a profile; give profile to use one by its name (one that somebody signed in to, for example; it is made if it does not exist), or leave it out for a profile of your thread's own. If a browser already runs on that profile you get that one: share it, and work in tabs you open yourself. preset \"window\" starts it with a window on a display, for a site that turns away a browser without one. The answer has the endpoint and, in attach, the command that connects to it with what is installed here. announce: you started a browser yourself, with an address for driving on 127.0.0.1; give its endpoint so that people see it. close: a browser you opened or announced, when you are done with it; a profile keeps its sign-ins. When a page needs a person (a sign-in, a code, a captcha), say so in your answer and name the browser: they open it from the conversation, do it, and tell you. Never type a password yourself that you were not given for that purpose.";
/// A note is a short label, not page content.
pub const NOTE_MAX: usize = 200;

/// The plain object schema offered to every harness.
#[must_use]
pub fn schema() -> Json {
    json!({"type":"object", "additionalProperties": false,
        "required":["command"], "properties": {
        "command":{"type":"string","enum":["list","open","announce","close"],"description":"List, open or join, announce, or close a browser."},
        "profile":{"type":"string","pattern":"^[a-z0-9][a-z0-9._-]{0,62}$","description":"Kept profile name; open makes it when missing. Omit for your thread's own."},
        "preset":{"type":"string","enum":["headless","window"],"description":"Full browser without a window, or on a display."},
        "url":{"type":"string","description":"First page of a newly opened browser."},
        "endpoint":{"type":"string","description":"The loopback HTTP address of a browser you started with --remote-debugging-port=0."},
        "browser":{"type":"string","description":"The identifier of a browser you opened or announced, to close."},
        "note":{"type":"string","maxLength":NOTE_MAX,"description":"A short line for people about the browser you open or announce."}
    }})
}
