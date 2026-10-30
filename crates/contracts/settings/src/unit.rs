//! Named commands and template programs supplied to managed units after each start.

use orch_plugin_contract::registration::{PluginHost, PluginKind, Restart};
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// The shipped command deadline, long enough for an ordinary package/tool setup.
pub const AFTER_UP_TIMEOUT_SECS: u32 = 300;
/// The explicitly configurable upper deadline: one hour.
pub const AFTER_UP_TIMEOUT_MAX_SECS: u32 = 3600;

/// One argv executed as the unit user; a shell is run only if explicitly named in argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AfterUpCommand {
    /// The program followed by its arguments; a nonempty program is required.
    #[schemars(length(min = 1))]
    pub command: Vec<String>,
    /// The deadline, from one second through one hour.
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 3600))]
    pub timeout_secs: u32,
}
const fn default_timeout() -> u32 {
    AFTER_UP_TIMEOUT_SECS
}

/// Named machine storages for owned data; absent members select the machine's default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitStorage {
    /// The workspace's storage, available to environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// The home data's storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// The private connector state storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Technical exclusions of a provider runtime, independent of where it runs.
#[must_use]
pub fn runtime_unavailable(member: &str) -> Option<&'static str> {
    match member {
        "workspace" => Some("a provider runtime has no workspace"),
        "mounts" => Some("a provider runtime has no work mounts"),
        "shells" => Some("a provider runtime has no interactive environment shells"),
        "browsers" => Some("a provider runtime has no environment browsers"),
        "idle_warning_minutes" => Some("a provider runtime has no environment idle warning"),
        _ => None,
    }
}

/// A template or resolved layer's named command, ordered lexically within that layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamedAfterUpCommand {
    /// Its stable name; also the only command text retained in history.
    pub name: String,
    /// Literal program and arguments, without an implicit shell.
    #[schemars(length(min = 1))]
    pub command: Vec<String>,
    /// The same per-command deadline used by the scoped map.
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 3600))]
    pub timeout_secs: u32,
}
impl NamedAfterUpCommand {
    /// Names one argv without changing its deadline.
    #[must_use]
    pub fn new(name: String, run: AfterUpCommand) -> Self {
        Self {
            name,
            command: run.command,
            timeout_secs: run.timeout_secs,
        }
    }
}

impl AfterUpCommand {
    /// Rejects an empty executable, NUL arguments and unsupported deadlines before any execution.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .command
            .first()
            .is_none_or(|program| program.trim().is_empty())
        {
            return Err("a command needs a nonempty executable in argv".into());
        }
        if self.command.iter().any(|argument| argument.contains('\0')) {
            return Err("command arguments cannot contain NUL".into());
        }
        if !(1..=AFTER_UP_TIMEOUT_MAX_SECS).contains(&self.timeout_secs) {
            return Err("a command timeout is between 1 and 3600 seconds".into());
        }
        Ok(())
    }
}

/// Reads a settings map without running anything; null members remove inherited names.
pub fn setting_commands(value: &Json) -> Result<BTreeMap<String, Option<AfterUpCommand>>, String> {
    let commands: BTreeMap<String, Option<AfterUpCommand>> = serde_json::from_value(value.clone())
        .map_err(|_| "after_up is a map of named argv commands or null".to_owned())?;
    for (name, command) in &commands {
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return Err("a command name is nonempty and has no control characters".into());
        }
        if let Some(command) = command {
            command.validate()?;
        }
    }
    Ok(commands)
}

/// Reads a template's named list, converting its superseded init object into entry init.
/// Template commands remain a separate first layer from resolved scoped settings.
pub fn template_commands(template: &Json) -> Result<Vec<NamedAfterUpCommand>, String> {
    let mut commands: BTreeMap<String, AfterUpCommand> = BTreeMap::new();
    if let Some(list) = template.get("after_up") {
        let list: Vec<NamedAfterUpCommand> = serde_json::from_value(list.clone())
            .map_err(|_| "template after_up is a list of named argv commands".to_owned())?;
        for entry in list {
            if entry.name.trim().is_empty() || entry.name.chars().any(char::is_control) {
                return Err("a command name is nonempty and has no control characters".into());
            }
            let run = AfterUpCommand {
                command: entry.command,
                timeout_secs: entry.timeout_secs,
            };
            run.validate()?;
            if commands.insert(entry.name, run).is_some() {
                return Err("template command names must be unique".into());
            }
        }
    }
    if let Some(init) = template.get("init").filter(|value| !value.is_null()) {
        let init: AfterUpCommand = serde_json::from_value(init.clone()).map_err(|_| {
            "the superseded init entry needs argv and an optional timeout_secs".to_owned()
        })?;
        init.validate()?;
        commands.entry("init".into()).or_insert(init);
    }
    Ok(commands
        .into_iter()
        .map(|(name, run)| NamedAfterUpCommand::new(name, run))
        .collect())
}

/// The standalone schema embedded in driver template forms, without external definitions.
pub fn after_up_template_schema() -> Json {
    json!({"type":"array","title":"Commands after start","description":"Named repeatable commands run as the unit user before readiness, in name order. A failure fails the start.","default":[],"items":{"type":"object","additionalProperties":false,
    "required":["name","command"],"properties":{
        "name":{"type":"string","minLength":1,"title":"Command name","description":"A unique name; number prefixes control the execution order.","default":null,"examples":["10-cache"]},
        "command":{"type":"array","minItems":1,"items":{"type":"string"},"title":"Program and arguments","description":"An installed executable followed by literal arguments, without an implicit shell.","default":null,"examples":[["/usr/bin/mkdir","-p","/workspace/.cache"]]},
        "timeout_secs":{"type":"integer","minimum":1,"maximum":3600,"default":300,"title":"Time limit","description":"Maximum running time in seconds, from 1 to 3600.","x-unit":"seconds"}
    }}})
}

/// A program shipped by a provider template. The managed connector reads this as a plugin registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplatePlugin {
    /// One of the connector's provider, translator, adapter or executor extension points.
    pub kind: PluginKind,
    /// Its unique name, distinct from built-in implementations of its kind.
    pub name: String,
    /// The program in the template image and its literal arguments.
    pub command: Vec<String>,
    /// Variables for this program, inherited in addition to the unit's own variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// never, on-failure (the default), or always.
    #[serde(default)]
    pub restart: Restart,
}

/// Validates both unit kinds before a template is saved or used to make a unit.
pub fn template_plugins(template: &Json, provider: bool) -> Result<Vec<TemplatePlugin>, String> {
    template_commands(template)?;
    let Some(plugins) = template.get("plugins") else {
        return Ok(Vec::new());
    };
    if !provider {
        return Err("programs belong only to provider templates".into());
    }
    let plugins: Vec<TemplatePlugin> = serde_json::from_value(plugins.clone()).map_err(|_| {
        "plugins is a list of kind, name, command, env and restart registrations".to_owned()
    })?;
    let mut names = std::collections::BTreeSet::new();
    for plugin in &plugins {
        if plugin.kind.host() != PluginHost::Connector {
            return Err(
                "a managed connector hosts providers, translators, adapters and executors".into(),
            );
        }
        if plugin.name.trim().is_empty()
            || plugin.name.chars().any(char::is_control)
            || !names.insert(plugin.name.as_str())
        {
            return Err(
                "template program names are nonempty, unique and have no control characters".into(),
            );
        }
        let programs = orch_agent_programs::Programs::builtin();
        let reserved = match plugin.kind.as_str() {
            "adapter" => programs.kinds().any(|kind| kind == plugin.name),
            "executor" => programs.executor(&plugin.name).is_some(),
            "provider" => {
                orch_provider_contract::choice::builtin::ALL.contains(&plugin.name.as_str())
            }
            _ => false,
        };
        if reserved {
            return Err(
                "a template program cannot replace a built-in implementation of its kind".into(),
            );
        }
        AfterUpCommand {
            command: plugin.command.clone(),
            timeout_secs: AFTER_UP_TIMEOUT_SECS,
        }
        .validate()?;
        if plugin.env.iter().any(|(name, value)| {
            name.is_empty() || name.contains(['=', '\0']) || value.contains('\0')
        }) {
            return Err("a program's variable names cannot be empty or contain equals or NUL; values cannot contain NUL".into());
        }
    }
    Ok(plugins)
}

/// The provider-only form schema permits every connector plugin kind.
pub fn plugins_template_schema() -> Json {
    json!({"type":"array","title":"Registered programs","description":"Programs installed in the runtime image that implement connector interfaces.","default":[],"items":{"type":"object","additionalProperties":false,
    "required":["kind","name","command"],"properties":{
        "kind":{"enum":["provider","translator","adapter","executor"],"title":"Program kind","description":"The connector interface implemented by this program.","default":null},
        "name":{"type":"string","minLength":1,"title":"Registered name","description":"A unique name that cannot replace a built-in implementation.","default":null,"examples":["team-provider"]},
        "command":{"type":"array","minItems":1,"items":{"type":"string"},"title":"Program and arguments","description":"The installed program and literal arguments for the plugin transport.","default":null,"examples":[["/opt/team/provider","--stdio"]]},
        "env":{"type":"object","additionalProperties":{"type":"string"},"title":"Program variables","description":"Additional plain variables; keep secrets out of this visible map.","default":{}},
        "restart":{"enum":["never","on-failure","always"],"default":"on-failure","title":"Restart policy","description":"Choose whether to restart failed programs, every exited program, or none."}
    }}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_and_deadlines_are_checked_and_scoped_nulls_remove_names() {
        assert!(
            setting_commands(&json!({"10-tool":{"command":["tool","a b"]},"20-inherited":null}))
                .is_ok()
        );
        for value in [
            json!({"x":{"command":[]}}),
            json!({"x":{"command":[""]}}),
            json!({"x":{"command":["a\u{0}b"]}}),
            json!({"x":{"command":["tool"],"timeout_secs":0}}),
            json!({"x":{"command":["tool"],"timeout_secs":3601}}),
            json!({"x":{"command":["tool"],"tolerate":true}}),
        ] {
            assert!(setting_commands(&value).is_err());
        }
    }
    #[test]
    fn template_names_order_commands_and_legacy_init_is_one_named_entry() {
        let list=template_commands(&json!({"after_up":[{"name":"20-cache","command":["cache"]},{"name":"10-tools","command":["tools"]}],"init":{"command":["old-init"]}})).unwrap();
        assert_eq!(
            list.iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["10-tools", "20-cache", "init"]
        );
        assert!(template_commands(&json!({"after_up":[{"name":"same","command":["a"]},{"name":"same","command":["b"]}]})).is_err());
    }
    #[test]
    fn provider_programs_allow_all_connector_kinds_but_cannot_replace_builtins_or_run_on_environment_templates()
     {
        for kind in ["provider", "translator", "adapter", "executor"] {
            let template = json!({"plugins":[{"kind":kind,"name":"my-program","command":["/opt/my-program"]}]});
            assert!(template_plugins(&template, true).is_ok());
            assert!(template_plugins(&template, false).is_err());
        }
        for plugins in [
            json!([{"kind":"driver","name":"foreign","command":["x"]}]),
            json!([{"kind":"adapter","name":"codex","command":["x"]}]),
            json!([{"kind":"provider","name":"same","command":["x"]},{"kind":"translator","name":"same","command":["x"]}]),
            json!([{"kind":"provider","name":"x","command":[]}]),
        ] {
            assert!(template_plugins(&json!({"plugins":plugins}), true).is_err());
        }
    }
}
