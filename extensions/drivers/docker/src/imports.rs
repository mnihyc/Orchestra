//! Proposing a spec from a Dockerfile, an image reference or an archive, with a note for what cannot be carried over.
//!
//! An import reads something a person already has and proposes the spec of a template for it; it saves and makes
//! nothing. What is read is what a unit made from it would need to know: the user the image runs as, the volumes
//! it declares (which a unit's volumes do not replace), the command it starts by itself (which a unit does not run:
//! its main process is the connector), the base images a Dockerfile starts from, the architecture of an image.
//! What cannot be carried over is a note, from a plain remark to a blocker such as an image no host here can run.
//!
//! A Dockerfile is read by its instructions alone, without the daemon; an image is read from the host when it has
//! it and is pulled to be read when it does not; an archive is read from the manifest and configuration it holds.

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::imports::{ImportNote, ImportProposal, NoteLevel};
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::sources::DOCKERFILE_MAX_BYTES;
use orch_types::{Hash, Json};
use serde_json::json;

use crate::archive::{read_image_archive, spool};
use crate::driver::DockerDriver;
use crate::images::{digest_of, parse_reference};
use crate::progress::Silent;

/// One instruction of a Dockerfile: its keyword in capitals, and the rest of its (joined) line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// The keyword, in capitals.
    pub keyword: String,
    /// What follows it.
    pub arguments: String,
}

/// Reads the instructions of a Dockerfile: comments and blank lines are left out, and a line that ends in a
/// backslash goes on in the next. The escape character a `# escape=` directive names is not honoured; a Dockerfile
/// that changes it is a Windows one.
#[must_use]
pub fn instructions(dockerfile: &str) -> Vec<Instruction> {
    let mut found = Vec::new();
    let mut joined = String::new();
    for line in dockerfile.lines() {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if joined.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        // A comment line inside a continued instruction is dropped and does not end it.
        if !joined.is_empty() && trimmed.starts_with('#') {
            continue;
        }
        if let Some(without) = trimmed.strip_suffix('\\') {
            joined.push_str(without);
            joined.push(' ');
            continue;
        }
        joined.push_str(trimmed);
        let text = std::mem::take(&mut joined);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let (keyword, arguments) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        found.push(Instruction {
            keyword: keyword.to_ascii_uppercase(),
            arguments: arguments.trim().to_owned(),
        });
    }
    let text = joined.trim();
    if !text.is_empty() {
        let (keyword, arguments) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        found.push(Instruction {
            keyword: keyword.to_ascii_uppercase(),
            arguments: arguments.trim().to_owned(),
        });
    }
    found
}

/// What a Dockerfile says that matters to a unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockerfileFacts {
    /// The images it starts from, in order, without the names of its own stages and without `scratch`.
    pub bases: Vec<String>,
    /// The last `USER`.
    pub user: Option<String>,
    /// Each `VOLUME`.
    pub volumes: Vec<String>,
    /// The last `ENTRYPOINT`.
    pub entrypoint: Option<String>,
    /// The last `CMD`.
    pub cmd: Option<String>,
}

/// The paths of a `VOLUME` instruction: a JSON list or words.
fn volume_paths(arguments: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<String>>(arguments) {
        Ok(list) => list,
        Err(_) => arguments.split_whitespace().map(str::to_owned).collect(),
    }
}

/// Reads the facts out of a Dockerfile.
#[must_use]
pub fn read_dockerfile(dockerfile: &str) -> DockerfileFacts {
    let mut facts = DockerfileFacts::default();
    let mut stages: Vec<String> = Vec::new();
    for instruction in instructions(dockerfile) {
        match instruction.keyword.as_str() {
            "FROM" => {
                let words: Vec<&str> = instruction
                    .arguments
                    .split_whitespace()
                    .filter(|word| !word.starts_with("--"))
                    .collect();
                let Some(&base) = words.first() else { continue };
                let stage = (words
                    .get(1)
                    .is_some_and(|word| word.eq_ignore_ascii_case("as")))
                .then(|| words.get(2).map(|name| name.to_ascii_lowercase()))
                .flatten();
                // A stage name starts a new image: what `USER` and the rest said before it is not this one's.
                facts.user = None;
                facts.entrypoint = None;
                facts.cmd = None;
                facts.volumes.clear();
                let is_stage = stages.iter().any(|name| name.eq_ignore_ascii_case(base));
                if !is_stage && !base.eq_ignore_ascii_case("scratch") {
                    facts.bases.push(base.to_owned());
                }
                if let Some(stage) = stage {
                    stages.push(stage);
                }
            }
            "USER" => facts.user = Some(instruction.arguments),
            "VOLUME" => facts.volumes.extend(volume_paths(&instruction.arguments)),
            "ENTRYPOINT" => facts.entrypoint = Some(instruction.arguments),
            "CMD" => facts.cmd = Some(instruction.arguments),
            _ => {}
        }
    }
    facts
}

/// The base images of a Dockerfile that a build pulls: those whose name is known before the build (a name made of
/// an argument is not).
#[must_use]
pub fn base_references(dockerfile: &str) -> Vec<String> {
    read_dockerfile(dockerfile)
        .bases
        .into_iter()
        .filter(|base| !base.contains('$'))
        .collect()
}

fn note(level: NoteLevel, text: impl Into<String>) -> ImportNote {
    ImportNote {
        level,
        text: text.into(),
    }
}

/// The spec a new template starts with: the driver's defaults for resources, network, security, sandbox and
/// approval, and the standard places for the workspace and the home. A provider's runtime gets the confined
/// defaults of the shipped runtime template.
#[must_use]
pub fn default_spec(kind: RuntimeKind) -> serde_json::Map<String, Json> {
    let value = match kind {
        RuntimeKind::Environment => json!({
            "runtime": "runc",
            "workspace_mount": "/workspace",
            "home_mount": "/home/agent",
            "resources": { "cpu_millis": 4000, "memory_mb": 8192, "pids": 4096, "disk_gb": 50 },
            "resources_max": { "cpu_millis": 16000, "memory_mb": 65536, "pids": 32768, "disk_gb": 500 },
            "needs": { "memory_mb": 2048, "cpu_millis": 1000, "disk_gb": 10 },
            "network": { "mode": "bridge", "egress": "open", "dns": [] },
            "security": { "read_only_rootfs": false, "no_new_privileges": false, "cap_add": [], "cap_drop": [] },
            "base_capabilities": [],
            "allowed_capabilities": [],
            "sandbox": { "default": "open", "max": "open" },
            "agent_defaults": { "approval.mode": "on_request" },
            "env": {},
            "mounts": [],
            "exec": { "offers": ["codex-exec-server", "claude-code", "plain"], "orch_tools": true },
            "colocated_harnesses": [],
            "mcp_server_ids": [],
            "init": null,
            "host_labels": []
        }),
        RuntimeKind::Provider => json!({
            "runtime": "runc",
            "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512 },
            "network": { "mode": "bridge", "egress": "open", "dns": [] },
            "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] },
            "env": {},
            "mounts": [ { "type": "tmpfs", "target": "/tmp" } ],
            "host_labels": []
        }),
    };
    match value {
        Json::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

/// The user of an image as the `user` of a spec: numbers when the image gives numbers, a name when it gives a
/// name, and nothing when it gives none (an image without a user runs as root).
fn user_member(user: &str) -> Option<Json> {
    let user = user.trim();
    if user.is_empty() {
        return None;
    }
    let (name, group) = user
        .split_once(':')
        .map_or((user, None), |(u, g)| (u, Some(g)));
    if let Ok(uid) = name.parse::<u32>() {
        let gid = group
            .and_then(|group| group.parse::<u32>().ok())
            .unwrap_or(uid);
        return Some(json!({ "uid": uid, "gid": gid }));
    }
    Some(json!({ "name": name }))
}

/// The notes every import of an image gives about its user, its volumes and the command it starts by itself.
fn image_notes(
    user: Option<&str>,
    volumes: &[String],
    entrypoint: Option<&str>,
    cmd: Option<&str>,
    notes: &mut Vec<ImportNote>,
) {
    match user.map(str::trim).filter(|user| !user.is_empty()) {
        Some(user) => {
            notes.push(note(NoteLevel::Info, format!("the last USER is {user}")));
            if user.parse::<u32>().is_err() && !user.split(':').next().is_some_and(|u| u.parse::<u32>().is_ok()) {
                notes.push(note(
                    NoteLevel::Warning,
                    format!("the user {user} is given by name only; it is looked up in the image when the unit is first made, and a user that is not there fails that"),
                ));
            }
        }
        None => notes.push(note(
            NoteLevel::Info,
            "the image names no user: its processes run as root unless the template says another user",
        )),
    }
    for volume in volumes {
        notes.push(note(
            NoteLevel::Warning,
            format!("{volume} is declared as a volume: it will not outlive the unit unless you add a mount for it"),
        ));
    }
    for (what, command) in [("entrypoint", entrypoint), ("command", cmd)] {
        if let Some(command) = command.filter(|command| !command.trim().is_empty()) {
            notes.push(note(
                NoteLevel::Warning,
                format!(
                    "this image starts `{}` by itself as its {what}; here it will not, see `init`",
                    command.trim()
                ),
            ));
        }
    }
}

/// Proposes a spec from a Dockerfile given as text. `context` is the spec's `context` member when the person
/// gave a context archive or a Git address.
pub fn propose_dockerfile(
    kind: RuntimeKind,
    dockerfile: &str,
    context: Option<Json>,
) -> Result<ImportProposal, DriverError> {
    if dockerfile.trim().is_empty() {
        return Err(DriverError::InvalidSpec(
            "the Dockerfile is empty".to_owned(),
        ));
    }
    if dockerfile.len() > DOCKERFILE_MAX_BYTES {
        return Err(DriverError::InvalidSpec(format!(
            "the Dockerfile is over {} KiB",
            DOCKERFILE_MAX_BYTES / 1024
        )));
    }
    let facts = read_dockerfile(dockerfile);
    let mut notes = Vec::new();
    for base in &facts.bases {
        notes.push(note(NoteLevel::Info, format!("it is built FROM {base}")));
    }
    if facts.bases.is_empty() {
        notes.push(note(
            NoteLevel::Warning,
            "no base image could be read from the FROM lines",
        ));
    }
    image_notes(
        facts.user.as_deref(),
        &facts.volumes,
        facts.entrypoint.as_deref(),
        facts.cmd.as_deref(),
        &mut notes,
    );
    if context.is_none()
        && instructions(dockerfile).iter().any(|i| {
            matches!(i.keyword.as_str(), "COPY" | "ADD")
                && !i.arguments.contains("--from=")
                && !i.arguments.contains("http")
        })
    {
        notes.push(note(
            NoteLevel::Warning,
            "the Dockerfile copies files in and no context was given: the build will fail unless they are in the Dockerfile's own directory",
        ));
    }
    let mut spec = default_spec(kind);
    spec.insert(
        "build".to_owned(),
        json!({ "dockerfile": dockerfile, "context": context, "args": {}, "target": null }),
    );
    if let Some(user) = facts.user.as_deref().and_then(user_member) {
        spec.insert("user".to_owned(), user);
    }
    Ok(ImportProposal {
        spec: Json::Object(spec),
        notes,
    })
}

/// The architecture names the daemon and the registries use for one another.
#[must_use]
pub fn architecture_matches(image: &str, host: &str) -> bool {
    fn normal(name: &str) -> &str {
        match name {
            "x86_64" | "amd64" | "x86-64" => "amd64",
            "aarch64" | "arm64" => "arm64",
            other => other,
        }
    }
    normal(image) == normal(host)
}

/// What the configuration of an image says, in the terms of an import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageFacts {
    /// The user.
    pub user: Option<String>,
    /// The volumes it declares.
    pub volumes: Vec<String>,
    /// The entrypoint, as one line.
    pub entrypoint: Option<String>,
    /// The command, as one line.
    pub cmd: Option<String>,
    /// The architecture.
    pub architecture: Option<String>,
    /// The operating system.
    pub os: Option<String>,
    /// The size, in bytes.
    pub size: Option<i64>,
}

impl ImageFacts {
    /// Reads the configuration of an image in the form both the daemon and an archive give it: the members
    /// `Config` (with `User`, `Volumes`, `Entrypoint`, `Cmd`), `architecture`/`Architecture`, `os`/`Os`.
    #[must_use]
    pub fn from_config(config: &Json) -> Self {
        let inner = config
            .get("config")
            .or_else(|| config.get("Config"))
            .unwrap_or(&Json::Null);
        let text = |value: Option<&Json>| value.and_then(Json::as_str).map(str::to_owned);
        let line = |value: Option<&Json>| {
            let list = value?.as_array()?;
            let words: Vec<&str> = list.iter().filter_map(Json::as_str).collect();
            (!words.is_empty()).then(|| words.join(" "))
        };
        let get = |a: &str, b: &str| config.get(a).or_else(|| config.get(b));
        Self {
            user: text(inner.get("User")).filter(|user| !user.is_empty()),
            volumes: inner
                .get("Volumes")
                .and_then(Json::as_object)
                .map(|volumes| volumes.keys().cloned().collect())
                .unwrap_or_default(),
            entrypoint: line(inner.get("Entrypoint")),
            cmd: line(inner.get("Cmd")),
            architecture: text(get("architecture", "Architecture")),
            os: text(get("os", "Os")),
            size: get("size", "Size").and_then(Json::as_i64),
        }
    }
}

/// Proposes a spec from what is known of an image: where it comes from (`source` is the members of the spec that
/// name it) and what its configuration says. `host_architecture` is the architecture of this host.
#[must_use]
pub fn propose_image(
    kind: RuntimeKind,
    source: serde_json::Map<String, Json>,
    facts: &ImageFacts,
    host_architecture: &str,
) -> ImportProposal {
    let mut notes = Vec::new();
    image_notes(
        facts.user.as_deref(),
        &facts.volumes,
        facts.entrypoint.as_deref(),
        facts.cmd.as_deref(),
        &mut notes,
    );
    if let Some(architecture) = facts.architecture.as_deref()
        && !host_architecture.is_empty()
        && !architecture_matches(architecture, host_architecture)
    {
        notes.push(note(
            NoteLevel::Blocker,
            format!("the image is for {architecture} and this host runs {host_architecture}; no host that is known here can run it"),
        ));
    }
    if let Some(os) = facts.os.as_deref().filter(|os| *os != "linux") {
        notes.push(note(
            NoteLevel::Blocker,
            format!("the image is for {os}; only Linux images run here"),
        ));
    }
    if let Some(size) = facts.size.filter(|size| *size > 0) {
        notes.push(note(
            NoteLevel::Info,
            format!("the image is {} MiB", size / (1 << 20)),
        ));
    }
    let mut spec = default_spec(kind);
    spec.extend(source);
    if let Some(user) = facts.user.as_deref().and_then(user_member) {
        spec.insert("user".to_owned(), user);
    }
    ImportProposal {
        spec: Json::Object(spec),
        notes,
    }
}

/// The members of a spec that name a registry image, pinned to the digest found when there is one.
#[must_use]
pub fn image_source(reference: &str, digest: Option<&str>) -> serde_json::Map<String, Json> {
    let mut source = serde_json::Map::new();
    source.insert("image".to_owned(), json!(reference));
    source.insert(
        "image_digest".to_owned(),
        digest.map_or(Json::Null, |digest| json!(digest)),
    );
    source.insert("build".to_owned(), Json::Null);
    source.insert("archive".to_owned(), Json::Null);
    source
}

/// The members of a spec that name an image archive.
#[must_use]
pub fn archive_source(blob: &str, image: Option<&str>) -> serde_json::Map<String, Json> {
    let mut source = serde_json::Map::new();
    source.insert("image".to_owned(), Json::Null);
    source.insert("image_digest".to_owned(), Json::Null);
    source.insert("build".to_owned(), Json::Null);
    source.insert(
        "archive".to_owned(),
        json!({ "blob": blob, "image": image }),
    );
    source
}

/// A reference without a tag or a digest is the tag `latest`; a note says so, since the proposal pins to a digest.
#[must_use]
pub fn reference_note(reference: &str) -> Option<ImportNote> {
    let parsed = parse_reference(reference);
    (parsed.tag.is_none() && parsed.digest.is_none()).then(|| {
        note(
            NoteLevel::Info,
            format!("{reference} names no tag; the tag latest is meant"),
        )
    })
}

/// What the daemon's inspection of an image says, in the terms of an import.
#[must_use]
pub fn facts_of_inspection(image: &bollard::models::ImageInspect) -> ImageFacts {
    let line = |words: Option<&Vec<String>>| {
        words
            .filter(|words| !words.is_empty())
            .map(|words| words.join(" "))
    };
    let config = image.config.as_ref();
    ImageFacts {
        user: config
            .and_then(|config| config.user.clone())
            .filter(|user| !user.is_empty()),
        volumes: config
            .and_then(|config| config.volumes.clone())
            .unwrap_or_default(),
        entrypoint: line(config.and_then(|config| config.entrypoint.as_ref())),
        cmd: line(config.and_then(|config| config.cmd.as_ref())),
        architecture: image.architecture.clone(),
        os: image.os.clone(),
        size: image.size,
    }
}

/// Proposes a spec for an image the host holds, from the daemon's inspection of it.
#[must_use]
pub fn propose_from_inspection(
    kind: RuntimeKind,
    reference: &str,
    image: &bollard::models::ImageInspect,
    host_architecture: &str,
) -> ImportProposal {
    let facts = facts_of_inspection(image);
    let digest = digest_of(image, reference);
    let mut proposal = propose_image(
        kind,
        image_source(reference, digest.as_deref()),
        &facts,
        host_architecture,
    );
    if let Some(digest) = &digest {
        proposal.notes.push(note(
            NoteLevel::Info,
            format!("the image is pinned to {digest}; clear the pin to follow the tag"),
        ));
    }
    proposal.notes.extend(reference_note(reference));
    proposal
}

impl DockerDriver {
    /// Proposes a spec from an image reference: the image is read from the host when it has it, and pulled to be
    /// read when it does not (the proposal says so).
    pub(crate) async fn propose_image_import(
        &self,
        kind: RuntimeKind,
        reference: &str,
    ) -> Result<ImportProposal, DriverError> {
        let (image, pulled) = match self.daemon.image(reference).await? {
            Some(image) => (image, false),
            None => {
                self.pull_image(reference, None, &Silent).await?;
                let image = self.daemon.image(reference).await?.ok_or_else(|| {
                    DriverError::Image(format!(
                        "{reference} is not on this host after it was pulled"
                    ))
                })?;
                (image, true)
            }
        };
        let mut proposal =
            propose_from_inspection(kind, reference, &image, &self.facts().architecture);
        if pulled {
            proposal.notes.push(note(
                NoteLevel::Info,
                "the image was pulled to this host to read its configuration",
            ));
        }
        Ok(proposal)
    }

    /// Proposes a spec from an image archive held as a blob.
    pub(crate) async fn propose_archive_import(
        &self,
        kind: RuntimeKind,
        blob: &str,
        which: Option<&str>,
    ) -> Result<ImportProposal, DriverError> {
        let hash: Hash = blob
            .parse()
            .map_err(|_| DriverError::InvalidSpec(format!("{blob:?} is not the hash of a blob")))?;
        let source = self.blobs.as_ref().ok_or_else(|| {
            DriverError::Image("this host has no way to fetch the archive".to_owned())
        })?;
        let file = spool(source.as_ref(), &hash).await?;
        let path = file.path().to_owned();
        let architecture = self.facts().architecture.clone();
        let images = tokio::task::spawn_blocking(move || read_image_archive(&path, &architecture))
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))??;
        let chosen = match which {
            Some(wanted) => images
                .iter()
                .find(|image| image.names.iter().any(|name| name == wanted))
                .ok_or_else(|| {
                    DriverError::InvalidSpec(format!("the archive holds no image named {wanted}"))
                })?,
            None if images.len() == 1 => &images[0],
            None => {
                let names: Vec<String> = images
                    .iter()
                    .flat_map(|image| image.names.clone())
                    .collect();
                return Err(DriverError::InvalidSpec(format!(
                    "the archive holds {} images ({}); name one",
                    images.len(),
                    names.join(", ")
                )));
            }
        };
        let facts = ImageFacts::from_config(&chosen.config);
        let mut proposal = propose_image(
            kind,
            archive_source(blob, which),
            &facts,
            &self.facts().architecture,
        );
        for name in &chosen.names {
            proposal.notes.insert(
                0,
                note(NoteLevel::Info, format!("the archive holds {name}")),
            );
        }
        Ok(proposal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCKERFILE: &str = "# build the thing\nFROM --platform=linux/amd64 debian:bookworm-slim AS base\nRUN apt-get update \\\n  && apt-get install -y curl\n# a comment\nFROM base AS final\nFROM scratch\nFROM busybox:1.37\nVOLUME [\"/data\", \"/cache\"]\nVOLUME /logs\nUSER agent\nENTRYPOINT [\"/bin/server\"]\nCMD [\"--port\", \"80\"]\n";

    #[test]
    fn a_dockerfile_is_read_by_its_instructions_with_continuations_and_comments() {
        let list = instructions(DOCKERFILE);
        assert_eq!(
            list[0],
            Instruction {
                keyword: "FROM".into(),
                arguments: "--platform=linux/amd64 debian:bookworm-slim AS base".into()
            }
        );
        assert_eq!(list[1].keyword, "RUN");
        assert!(
            list[1].arguments.contains("apt-get update")
                && list[1].arguments.contains("apt-get install -y curl")
        );
        assert!(!list.iter().any(|i| i.arguments.contains("a comment")));
        assert_eq!(list.last().unwrap().keyword, "CMD");
    }

    #[test]
    fn the_images_a_build_starts_from_are_those_that_are_not_its_own_stages() {
        let facts = read_dockerfile(DOCKERFILE);
        assert_eq!(facts.bases, ["debian:bookworm-slim", "busybox:1.37"]);
        assert_eq!(base_references("FROM ${BASE}\nFROM alpine"), ["alpine"]);
    }

    #[test]
    fn what_matters_to_a_unit_is_that_of_the_last_image_of_the_file() {
        let facts = read_dockerfile(DOCKERFILE);
        assert_eq!(facts.user.as_deref(), Some("agent"));
        assert_eq!(facts.volumes, ["/data", "/cache", "/logs"]);
        assert_eq!(facts.entrypoint.as_deref(), Some("[\"/bin/server\"]"));
        let staged = read_dockerfile("FROM a\nUSER root\nVOLUME /x\nFROM b\nRUN true\n");
        assert_eq!(
            staged.user, None,
            "the user of an earlier stage is not the image's"
        );
        assert!(staged.volumes.is_empty());
    }

    #[test]
    fn a_dockerfile_is_proposed_as_a_build_with_a_note_for_each_volume_the_entrypoint_and_a_user_by_name()
     {
        let proposal = propose_dockerfile(RuntimeKind::Environment, DOCKERFILE, None).unwrap();
        assert_eq!(proposal.spec["build"]["dockerfile"], DOCKERFILE);
        assert_eq!(proposal.spec["user"], json!({ "name": "agent" }));
        assert_eq!(proposal.spec["workspace_mount"], "/workspace");
        assert_eq!(proposal.spec["resources"]["memory_mb"], 8192);
        let warnings: Vec<_> = proposal
            .notes
            .iter()
            .filter(|n| n.level == NoteLevel::Warning)
            .map(|n| n.text.as_str())
            .collect();
        assert!(warnings.iter().any(|w| w.contains("/data is declared as a volume: it will not outlive the unit unless you add a mount for it")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("/logs is declared as a volume"))
        );
        assert!(warnings.iter().any(|w| w.contains(
            "starts `[\"/bin/server\"]` by itself as its entrypoint; here it will not, see `init`"
        )));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("agent is given by name only"))
        );
        assert!(
            proposal
                .notes
                .iter()
                .any(|n| n.level == NoteLevel::Info && n.text == "the last USER is agent")
        );
        assert!(!proposal.is_blocked());
    }

    #[test]
    fn a_dockerfile_that_copies_files_without_a_context_is_said_to_need_one() {
        let without =
            propose_dockerfile(RuntimeKind::Provider, "FROM alpine\nCOPY app /app\n", None)
                .unwrap();
        assert!(
            without
                .notes
                .iter()
                .any(|n| n.text.contains("no context was given"))
        );
        let with = propose_dockerfile(
            RuntimeKind::Provider,
            "FROM alpine\nCOPY app /app\n",
            Some(json!({ "blob": "0".repeat(64) })),
        )
        .unwrap();
        assert!(
            !with
                .notes
                .iter()
                .any(|n| n.text.contains("no context was given"))
        );
        assert_eq!(with.spec["build"]["context"]["blob"], "0".repeat(64));
        assert_eq!(
            with.spec["security"]["read_only_rootfs"], true,
            "a provider's runtime starts confined"
        );
    }

    #[test]
    fn an_empty_or_oversized_dockerfile_proposes_nothing() {
        assert!(propose_dockerfile(RuntimeKind::Environment, "  \n", None).is_err());
        let long = format!("FROM x\n#{}", "a".repeat(DOCKERFILE_MAX_BYTES));
        assert!(propose_dockerfile(RuntimeKind::Environment, &long, None).is_err());
    }

    #[test]
    fn an_image_is_proposed_with_its_user_as_numbers_and_a_blocker_for_another_architecture() {
        let config = json!({
            "architecture": "arm64", "os": "linux", "size": 52_428_800,
            "config": { "User": "1000:100", "Volumes": { "/var/lib/data": {} }, "Entrypoint": ["/entry.sh"], "Cmd": ["serve"] }
        });
        let facts = ImageFacts::from_config(&config);
        assert_eq!(facts.user.as_deref(), Some("1000:100"));
        assert_eq!(facts.volumes, ["/var/lib/data"]);
        assert_eq!(facts.entrypoint.as_deref(), Some("/entry.sh"));
        let proposal = propose_image(
            RuntimeKind::Environment,
            image_source("ghcr.io/o/x:1", Some("sha256:ab")),
            &facts,
            "x86_64",
        );
        assert_eq!(proposal.spec["user"], json!({ "uid": 1000, "gid": 100 }));
        assert_eq!(proposal.spec["image"], "ghcr.io/o/x:1");
        assert_eq!(proposal.spec["image_digest"], "sha256:ab");
        assert!(proposal.is_blocked());
        assert!(
            proposal
                .notes
                .iter()
                .any(|n| n.level == NoteLevel::Blocker && n.text.contains("arm64"))
        );
        assert!(
            !proposal
                .notes
                .iter()
                .any(|n| n.text.contains("given by name only")),
            "numbers need no lookup"
        );
        let same = propose_image(
            RuntimeKind::Environment,
            image_source("x", None),
            &ImageFacts {
                architecture: Some("amd64".into()),
                os: Some("linux".into()),
                ..ImageFacts::default()
            },
            "x86_64",
        );
        assert!(!same.is_blocked());
        assert!(same.notes.iter().any(|n| n.text.contains("names no user")));
        let windows = propose_image(
            RuntimeKind::Environment,
            image_source("x", None),
            &ImageFacts {
                os: Some("windows".into()),
                ..ImageFacts::default()
            },
            "x86_64",
        );
        assert!(windows.is_blocked());
    }

    #[test]
    fn the_architecture_of_an_image_and_of_a_host_are_compared_by_meaning() {
        assert!(
            architecture_matches("amd64", "x86_64") && architecture_matches("arm64", "aarch64")
        );
        assert!(!architecture_matches("arm64", "x86_64"));
    }

    #[test]
    fn a_reference_without_a_tag_is_said_to_mean_latest() {
        assert!(reference_note("busybox").is_some());
        assert!(reference_note("busybox:1.37").is_none());
        assert!(reference_note("busybox@sha256:abc").is_none());
    }
}
