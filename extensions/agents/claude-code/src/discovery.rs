//! Sessions found in a Claude Code home; the MCP servers of its own configuration, a user's and a directory's, by name, and one definition read out for an import.
//!
//! **Sessions.** Every transcript in the home's `projects` directory is a session the harness has. Its title is
//! what the harness wrote for it, or else what the person first asked; where it worked, and when it was last
//! used, come from the first lines of the file and from the file itself, so a home with thousands of sessions
//! is listed without reading them whole.
//!
//! **MCP servers.** The harness keeps the servers a person configured in its user file (`.claude.json`,
//! at the top for the user and under `projects` for one directory) and in the `.mcp.json` of a project. They
//! are listed by name, with a digest that changes when the definition does, and never with a command, an
//! address or a secret. One definition is read out when it is imported, and then it is a definition like any
//! other (`McpDefinition`): the values of variables whose names say they hold a secret are kept apart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::discovery::{
    DEFAULT_STARTUP_TIMEOUT_SECS, McpDefinition, McpFound, McpImportRequest, McpRequires, McpScope,
    McpSearch, McpServerSpec, McpTransport, McpWorkingDir, NativeSession,
};
use orch_plugin_contract::Secret;
use orch_types::{Hash, Json, Time};

use crate::config::KIND;
use crate::history::projects_directory;

/// How much of the beginning of a transcript is read to find its title and its directory: enough for the
/// first prompt, never the whole of a long session.
const HEAD_BYTES: usize = 128 * 1024;

/// The longest title a session gets.
const TITLE_CHARS: usize = 120;

/// The most sessions that are listed, the newest first.
const MOST_SESSIONS: usize = 500;

/// The names of variables whose values are secrets, by what they contain.
const SECRET_WORDS: [&str; 7] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "CREDENTIAL",
    "AUTH",
    "COOKIE",
];

// ---------------------------------------------------------------------------------------------------------
// Sessions

/// Text a transcript holds for the harness's own purposes, which is not what a person asked.
fn is_framing(text: &str) -> bool {
    let start = text.trim_start();
    start.starts_with("<system-reminder>")
        || start.starts_with("<command-")
        || start.starts_with("<local-command")
        || start.starts_with("Caveat:")
        || start.starts_with("[Request interrupted")
}

/// The text of a prompt in a transcript entry of the kind `user`, when it is one a person wrote.
pub(crate) fn prompt_text(entry: &Json) -> Option<String> {
    if entry.get("type")?.as_str()? != "user"
        || entry.get("isMeta").and_then(Json::as_bool).unwrap_or(false)
        || entry
            .get("isSidechain")
            .and_then(Json::as_bool)
            .unwrap_or(false)
    {
        return None;
    }
    let content = entry.get("message")?.get("content")?;
    let text = match content {
        Json::String(text) => text.clone(),
        Json::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Json::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Json::as_str))
            .filter(|text| !is_framing(text))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    (!text.trim().is_empty() && !is_framing(&text)).then_some(text)
}

fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    title
}

/// What the head of a transcript says of the session: its title, and where it worked.
fn read_head(path: &Path) -> (Option<String>, Option<String>) {
    use std::io::Read;
    let mut buffer = vec![0u8; HEAD_BYTES];
    let Ok(mut file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let read = file.read(&mut buffer).unwrap_or(0);
    buffer.truncate(read);
    let text = String::from_utf8_lossy(&buffer);
    let (mut named, mut asked, mut directory) = (None, None, None);
    for entry in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Json>(line).ok())
    {
        if named.is_none() {
            named = ["customTitle", "aiTitle", "summary"]
                .iter()
                .find_map(|member| {
                    entry
                        .get(*member)
                        .and_then(Json::as_str)
                        .filter(|title| !title.is_empty())
                        .map(title_from)
                });
        }
        if directory.is_none() {
            directory = entry.get("cwd").and_then(Json::as_str).map(str::to_owned);
        }
        if asked.is_none() {
            asked = prompt_text(&entry).map(|text| title_from(&text));
        }
        if named.is_some() && directory.is_some() {
            break;
        }
    }
    (named.or(asked), directory)
}

/// The sessions of a home, the most recently used first.
#[must_use]
pub fn sessions_of(home: &Path) -> Vec<NativeSession> {
    let mut found: Vec<(std::time::SystemTime, NativeSession)> = Vec::new();
    let Ok(projects) = std::fs::read_dir(projects_directory(home)) else {
        return Vec::new();
    };
    for project in projects
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
    {
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.filter_map(Result::ok) {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let modified = file
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let (title, cwd) = read_head(&path);
            found.push((
                modified,
                NativeSession {
                    native_session_ref: id.to_owned(),
                    title,
                    cwd,
                    updated_at: Some(Time::new(chrono::DateTime::<chrono::Utc>::from(modified))),
                    adopted_thread_id: None,
                },
            ));
        }
    }
    found.sort_by_key(|found| std::cmp::Reverse(found.0));
    found.truncate(MOST_SESSIONS);
    found.into_iter().map(|(_, session)| session).collect()
}

// ---------------------------------------------------------------------------------------------------------
// MCP servers

/// Where the harness keeps a user's own configuration: beside its home when it has one, else in the
/// directory of the person running it.
#[must_use]
pub fn user_configuration(
    home: Option<&Path>,
    environment: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    match home {
        Some(home) => Some(home.join(".claude.json")),
        None => environment
            .get("HOME")
            .map(|person| Path::new(person).join(".claude.json")),
    }
}

fn read_json(path: &Path) -> Option<Json> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn servers_in(value: Option<&Json>) -> Vec<(String, Json)> {
    value
        .and_then(Json::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, def)| (name.clone(), def.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn transport_of(definition: &Json) -> McpTransport {
    let by_type = definition.get("type").and_then(Json::as_str);
    if definition.get("command").is_some() || by_type == Some("stdio") {
        McpTransport::Stdio
    } else {
        McpTransport::Http
    }
}

/// Every server the harness has configured: the user's own, and for each directory asked about, what the
/// user's file holds for it and what the directory's `.mcp.json` holds.
#[must_use]
pub fn found_servers(user_file: Option<&Path>, search: &McpSearch) -> Vec<(McpFound, Json)> {
    let mut found = Vec::new();
    let user = user_file.and_then(read_json);
    let add = |found: &mut Vec<(McpFound, Json)>,
               name: String,
               def: Json,
               scope: McpScope,
               directory: Option<String>| {
        found.push((
            McpFound {
                definition_fingerprint: definition_from(&def)
                    .ok()
                    .map(|definition| definition.definition_fingerprint()),
                name,
                source: KIND.to_owned(),
                scope,
                directory,
                transport: transport_of(&def),
                digest: Hash::of(def.to_string().as_bytes()).to_string(),
            },
            def,
        ));
    };
    if let Some(user) = &user {
        for (name, def) in servers_in(user.get("mcpServers")) {
            add(&mut found, name, def, McpScope::User, None);
        }
    }
    for directory in &search.directories {
        if let Some(user) = &user {
            for (name, def) in servers_in(
                user.get("projects")
                    .and_then(|projects| projects.get(directory.as_str()))
                    .and_then(|project| project.get("mcpServers")),
            ) {
                add(
                    &mut found,
                    name,
                    def,
                    McpScope::Directory,
                    Some(directory.clone()),
                );
            }
        }
        if let Some(project) = read_json(&Path::new(directory).join(".mcp.json")) {
            for (name, def) in servers_in(project.get("mcpServers")) {
                if !found.iter().any(|(known, _)| {
                    known.name == name && known.directory.as_deref() == Some(directory.as_str())
                }) {
                    add(
                        &mut found,
                        name,
                        def,
                        McpScope::Directory,
                        Some(directory.clone()),
                    );
                }
            }
        }
    }
    found
}

fn holds_a_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// The definition of a server as the harness's file states it, in the shape Orch runs servers from.
pub fn definition_from(raw: &Json) -> Result<McpDefinition, AdapterError> {
    let transport = transport_of(raw);
    let mut env = BTreeMap::new();
    let mut secrets = BTreeMap::new();
    let mut secret_env = Vec::new();
    for (name, value) in raw
        .get("env")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
    {
        let Some(value) = value.as_str() else {
            continue;
        };
        if holds_a_secret(name) {
            secret_env.push(name.clone());
            secrets.insert(name.clone(), Secret::new(value));
        } else {
            env.insert(name.clone(), value.to_owned());
        }
    }
    let command = (transport == McpTransport::Stdio).then(|| {
        let mut argv = vec![
            raw.get("command")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
        ];
        argv.extend(
            raw.get("args")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_owned),
        );
        argv
    });
    let url = (transport == McpTransport::Http)
        .then(|| raw.get("url").and_then(Json::as_str).map(str::to_owned))
        .flatten();
    if transport == McpTransport::Http
        && raw
            .get("headers")
            .and_then(Json::as_object)
            .is_some_and(|headers| !headers.is_empty())
    {
        return Err(AdapterError::Unsupported(
            "an http MCP server that needs headers cannot be imported: a definition has no place for them"
                .to_owned(),
        ));
    }
    let definition = McpDefinition {
        spec: McpServerSpec {
            transport,
            command,
            url,
            authentication: None,
            env,
            secret_env,
            working_dir: McpWorkingDir::Workspace,
            startup_timeout_secs: DEFAULT_STARTUP_TIMEOUT_SECS,
            tools_allow: None,
            tools_deny: None,
            requires: McpRequires::default(),
        },
        secrets,
    };
    definition.check().map_err(|faults| AdapterError::Other {
        code: "mcp_definition_incomplete".to_owned(),
        message: faults
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        retryable: false,
    })?;
    Ok(definition)
}

/// Reads the definition of the server a request names out of the harness's configuration.
pub fn read_definition(
    user_file: Option<&Path>,
    request: &McpImportRequest,
) -> Result<McpDefinition, AdapterError> {
    let search = McpSearch {
        directories: request.directory.clone().into_iter().collect(),
    };
    let found = found_servers(user_file, &search);
    let raw = found
        .into_iter()
        .find(|(known, _)| {
            known.name == request.name
                && known.scope == request.scope
                && known.directory == request.directory
        })
        .map(|(_, raw)| raw)
        .ok_or_else(|| {
            AdapterError::NotFound(format!(
                "the harness has no server {:?} configured there",
                request.name
            ))
        })?;
    definition_from(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    fn example_server() -> Json {
        json!({ "command": "npx", "args": ["-y", "server"], "env": { "API_KEY": "s3cret", "REGION": "eu" } })
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_session_is_listed_with_what_the_person_first_asked_and_where_it_worked() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("projects/-workspace-web");
        write(
            &project.join("11111111-1111-4111-8111-111111111111.jsonl"),
            concat!(
                "{\"type\":\"queue-operation\",\"sessionId\":\"x\"}\n",
                "{\"type\":\"user\",\"cwd\":\"/workspace/web\",\"message\":{\"role\":\"user\",\"content\":[",
                "{\"type\":\"text\",\"text\":\"<system-reminder>ignore</system-reminder>\"},",
                "{\"type\":\"text\",\"text\":\"Fix the login redirect\\nand then the tests\"}]}}\n",
            ),
        );
        write(&project.join("notes.txt"), "not a session");
        write(
            &project.join("22222222-2222-4222-8222-222222222222.jsonl"),
            "{\"type\":\"summary\",\"summary\":\"Refactor billing\"}\n",
        );
        let sessions = sessions_of(home.path());
        assert_eq!(sessions.len(), 2);
        let first = sessions
            .iter()
            .find(|s| s.native_session_ref.starts_with("1111"))
            .unwrap();
        assert_eq!(first.title.as_deref(), Some("Fix the login redirect"));
        assert_eq!(first.cwd.as_deref(), Some("/workspace/web"));
        assert!(first.updated_at.is_some() && first.adopted_thread_id.is_none());
        let second = sessions
            .iter()
            .find(|s| s.native_session_ref.starts_with("2222"))
            .unwrap();
        assert_eq!(second.title.as_deref(), Some("Refactor billing"));
    }

    #[test]
    fn a_home_without_sessions_lists_none() {
        assert!(sessions_of(tempfile::tempdir().unwrap().path()).is_empty());
    }

    #[test]
    fn the_servers_of_a_users_file_and_of_a_directory_are_listed_by_name_and_without_their_secrets()
    {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let directory = project.path().display().to_string();
        write(
            &home.path().join(".claude.json"),
            &json!({
                "mcpServers": { "github": example_server() },
                "projects": { directory.clone(): { "mcpServers": { "local-db": { "type": "http", "url": "https://db.example/mcp" } } } }
            })
            .to_string(),
        );
        write(
            &project.path().join(".mcp.json"),
            &json!({ "mcpServers": { "tmux": { "command": "tmux-mcp" }, "local-db": { "command": "shadowed" } } }).to_string(),
        );
        let search = McpSearch {
            directories: vec![directory.clone()],
        };
        let found = found_servers(Some(&home.path().join(".claude.json")), &search);
        let summary: Vec<_> = found
            .iter()
            .map(|(f, _)| (f.name.as_str(), f.scope, f.transport, f.directory.is_some()))
            .collect();
        assert_eq!(
            summary,
            [
                ("github", McpScope::User, McpTransport::Stdio, false),
                ("local-db", McpScope::Directory, McpTransport::Http, true),
                ("tmux", McpScope::Directory, McpTransport::Stdio, true),
            ]
        );
        let listed =
            serde_json::to_string(&found.iter().map(|(f, _)| f).collect::<Vec<_>>()).unwrap();
        assert!(
            !listed.contains("s3cret") && !listed.contains("npx") && !listed.contains("db.example")
        );
        assert_eq!(found[0].0.source, "claude-code");
        assert_ne!(found[0].0.digest, found[1].0.digest);
    }

    #[test]
    fn a_definition_changes_its_digest_when_it_changes() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".claude.json");
        write(
            &file,
            &json!({ "mcpServers": { "a": { "command": "x" } } }).to_string(),
        );
        let before = found_servers(Some(&file), &McpSearch::default())[0]
            .0
            .digest
            .clone();
        write(
            &file,
            &json!({ "mcpServers": { "a": { "command": "y" } } }).to_string(),
        );
        assert_ne!(
            found_servers(Some(&file), &McpSearch::default())[0]
                .0
                .digest,
            before
        );
    }

    #[test]
    fn a_definition_read_out_keeps_the_secrets_apart() {
        let definition = definition_from(&example_server()).unwrap();
        assert_eq!(
            definition.spec.command.as_deref(),
            Some(&["npx".to_owned(), "-y".to_owned(), "server".to_owned()][..])
        );
        assert_eq!(
            definition.spec.env.get("REGION").map(String::as_str),
            Some("eu")
        );
        assert_eq!(definition.spec.secret_env, ["API_KEY"]);
        assert_eq!(definition.secrets["API_KEY"].expose(), "s3cret");
        assert!(!definition.spec.env.contains_key("API_KEY"));
        assert_eq!(definition.check(), Ok(()));
    }

    #[test]
    fn a_server_that_needs_headers_cannot_be_imported_and_one_that_is_not_there_is_not_found() {
        let with_headers = json!({ "type": "http", "url": "https://x.example/mcp", "headers": { "Authorization": "Bearer t" } });
        assert!(matches!(
            definition_from(&with_headers),
            Err(AdapterError::Unsupported(_))
        ));
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".claude.json");
        write(
            &file,
            &json!({ "mcpServers": { "github": example_server() } }).to_string(),
        );
        let request = |name: &str| McpImportRequest {
            server_id: new_id(),
            source: KIND.into(),
            scope: McpScope::User,
            name: name.into(),
            directory: None,
        };
        assert!(read_definition(Some(&file), &request("github")).is_ok());
        assert!(matches!(
            read_definition(Some(&file), &request("missing")),
            Err(AdapterError::NotFound(_))
        ));
    }
}
