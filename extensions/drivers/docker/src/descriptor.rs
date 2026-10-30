//! Docker template and provider schemas and confinement facts.
//!
//! The descriptor is what the rest of Orch knows of this driver and nothing else: it makes units for both pools,
//! confines the agent in a container, attaches mounts where they are asked for, makes private views where the
//! kernel can, creates volumes on named storages, can hold a unit still, and sources a unit's file system from a
//! registry, a Dockerfile or an archive. What depends on the daemon in front of it is read from the daemon when
//! the driver connects: which runtimes it has, whether it really limits what it is asked to, whether the kernel
//! has overlayfs, whether this host builds at all.

use std::collections::BTreeMap;

use orch_driver_contract::descriptor::{
    Confinement, DriverDescriptor, ProcessScope, Recreate, ResourceLimits,
};
use orch_driver_contract::imports::ImportDef;
use orch_driver_contract::mounts::MountPlacement;
use orch_driver_contract::network::EgressMode;
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::sources::{DOCKERFILE_MAX_BYTES, SourceKind};
use orch_types::{Field, FieldType, Json};
use serde_json::json;

use crate::capabilities;
use crate::client::DaemonFacts;
use crate::config::DockerConfig;

/// The driver's name, which a template names and which a capability's patch is keyed by.
pub const NAME: &str = "docker";

/// The version of the declaration.
pub const VERSION: &str = "1";

/// What the driver is called.
pub const TITLE: &str = "Docker containers";

/// The import of a Dockerfile.
pub const IMPORT_DOCKERFILE: &str = "dockerfile";

/// The import of an image from a registry.
pub const IMPORT_IMAGE: &str = "image";

/// The import of an image archive.
pub const IMPORT_ARCHIVE: &str = "archive";

fn nullable(schema: Json) -> Json {
    json!({ "anyOf": [schema, { "type": "null" }] })
}

fn resources_schema(title: &str) -> Json {
    json!({
        "title": title, "type": "object", "additionalProperties": false,
        "x-orch-storage-selection":true,
        "properties": {
            "cpu_millis": { "type": "integer", "minimum": 1, "description": "Thousandths of a processor" },
            "memory_mb": { "type": "integer", "minimum": 1, "description": "Memory, in MiB" },
            "memory_swap_mb": { "type": "integer", "minimum": 1 },
            "pids": { "type": "integer", "minimum": 1, "description": "Processes and threads" },
            "disk_gb": { "type": "integer", "minimum": 1, "description": "Disk, in GB" },
            "blkio_weight": { "type": "integer", "minimum": 10, "maximum": 1000 }
        }
    })
}

fn source_properties() -> Vec<(&'static str, Json)> {
    vec![
        (
            "image",
            json!({ "type": "string", "minLength": 1,
            "description": "A registry reference. Exactly one of image, build and archive is given" }),
        ),
        (
            "image_digest",
            nullable(
                json!({ "type": "string", "pattern": "^sha256:[0-9a-f]{64}$",
            "description": "Pins the image to this digest" }),
            ),
        ),
        (
            "build",
            nullable(json!({
            "type": "object", "required": ["dockerfile"], "additionalProperties": false,
            "description": "A Dockerfile, built on the host",
            "properties": {
                "dockerfile": { "type": "string", "minLength": 1, "maxLength": DOCKERFILE_MAX_BYTES, "x-multiline":true, "description": "The Dockerfile as text" },
                "context": nullable(json!({ "anyOf": [
                    { "type": "object", "required": ["blob"], "additionalProperties": false,
                      "x-choice":{"id":"archive","name":"Archive","description":"Use files from an uploaded build-context archive."},
                      "properties": { "blob": { "type": "string", "pattern": "^[0-9a-f]{64}$" } } },
                    { "type": "object", "required": ["git", "ref", "path"], "additionalProperties": false,
                      "x-choice":{"id":"git","name":"Git","description":"Use a public repository at a chosen revision and directory."},
                      "properties": { "git": { "type": "string" }, "ref": { "type": "string" }, "path": { "type": "string" } } } ] })),
                "args": { "type": "object", "additionalProperties": { "type": "string" },
                          "description": "Build arguments; stored and shown in clear, so never a secret" },
                "target": nullable(json!({ "type": "string" }))
            } })),
        ),
        (
            "archive",
            nullable(json!({
            "type": "object", "required": ["blob"], "additionalProperties": false,
            "description": "An image archive, loaded on the host",
            "properties": {
                "blob": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
                "image": nullable(json!({ "type": "string", "description": "Which of several images in the archive" }))
            } })),
        ),
    ]
}

fn common_properties() -> Vec<(&'static str, Json)> {
    let mut properties = source_properties();
    properties.extend([
        ("after_up", orch_settings_contract::unit::after_up_template_schema()),
        ("init", nullable(json!({"type":"object","deprecated":true,"description":"Superseded: runs as after_up entry init"}))),
        ("runtime", json!({ "type": "string", "x-runtime": true, "description": "A container runtime the host reports: runc, runsc, kata, …; the one an environment starts under unless its maker chooses another" })),
        ("user", json!({
            "type": "object", "additionalProperties": false,
            "properties": { "name": { "type": "string" }, "uid": { "type": "integer", "minimum": 0 }, "gid": { "type": "integer", "minimum": 0 } } })),
        ("resources", resources_schema("The most the unit may use")),
        ("network", json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "mode": { "enum": ["bridge", "none"], "description": "none reaches Orch alone, through the host's proxy" },
                "egress": { "enum": ["open", "restricted"], "description": "restricted reaches Orch and the other host names its place allows, through the host's proxy, and nothing else" },
                "dns": { "type": "array", "items": { "type": "string" } } } })),
        ("security", json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "read_only_rootfs": { "type": "boolean" }, "no_new_privileges": { "type": "boolean" },
                "cap_add": { "type": "array", "items": { "type": "string" } },
                "cap_drop": { "type": "array", "items": { "type": "string" } } } })),
        ("env", json!({ "type": "object", "additionalProperties": { "type": "string" } })),
        ("mounts", json!({ "type": "array", "items": {
            "type": "object", "required": ["type", "target"], "additionalProperties": false,
            "properties": {
                "type": { "enum": ["volume", "tmpfs", "bind"] },
                "source": { "type": "string" }, "target": { "type": "string" }, "read_only": { "type": "boolean" } } } })),
        ("host_labels", json!({ "type": "array", "items": { "type": "string" },
            "description": "A host must carry all of these" })),
    ]);
    properties
}

fn schema(title: &str, extra: Vec<(&'static str, Json)>) -> Json {
    let mut properties = serde_json::Map::new();
    for (name, schema) in common_properties().into_iter().chain(extra) {
        properties.insert(name.to_owned(), schema);
    }
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": title, "type": "object", "additionalProperties": false,
        "properties": properties,
        "oneOf": [
            {"required":["image"],"properties":{"image":{"type":"string","minLength":1}}},
            {"required":["build"],"properties":{"build":{"type":"object"}}},
            {"required":["archive"],"properties":{"archive":{"type":"object"}}}
        ]
    });
    describe_fields(&mut schema, "", &field_help());
    schema
}

/// The schema of the spec of a template for environments.
#[must_use]
pub fn environment_schema() -> Json {
    // Gateway grants belong in template capability lists even though they need no Docker patch.
    let capability_keys: Vec<&str> = capabilities::KEYS
        .into_iter()
        .chain(orch_driver_contract::capabilities::GATEWAY_GIVEN)
        .collect();
    let mut schema = schema(
        "A Docker container for an environment",
        vec![
            (
                "workspace_mount",
                json!({ "type": "string", "default": "/workspace", "description": "Where the workspace volume is mounted" }),
            ),
            (
                "home_mount",
                json!({ "type": "string", "default": "/home/agent", "description": "Where the home volume is mounted" }),
            ),
            (
                "resources_max",
                resources_schema("The most the limits may be raised to for one environment"),
            ),
            (
                "needs",
                resources_schema("What an environment declares it needs at least"),
            ),
            (
                "allowed_runtimes",
                json!({ "type": "array", "items": { "type": "string" } }),
            ),
            (
                "image_facts",
                json!({ "type": "object", "properties": { "image": { "type": "string" }, "docker": { "type": "boolean" }, "qemu": { "type": "boolean" } } }),
            ),
            (
                "base_capabilities",
                json!({ "type": "array", "items": { "enum": capability_keys.clone() } }),
            ),
            (
                "allowed_capabilities",
                json!({ "type": "array", "items": { "enum": capability_keys } }),
            ),
            (
                "sandbox",
                json!({ "type": "object", "properties": {
                "default": {"enum":["read_only","workspace","open"]},
                "max": {"enum":["read_only","workspace","open"]}
            } }),
            ),
            ("agent_defaults", json!({ "type": "object" })),
            (
                "exec",
                json!({ "type": "object", "properties": {
                "offers": {"type":"array","items":{"type":"string"}},
                "orch_tools": {"type":"boolean"}
            } }),
            ),
            (
                "colocated_harnesses",
                json!({ "type": "array", "items": { "type": "string" } }),
            ),
            (
                "mcp_server_ids",
                json!({ "type": "array", "items": { "type": "string" } }),
            ),
        ],
    );
    schema["x-orch-examples"] = examples(RuntimeKind::Environment);
    schema
}

/// Driver-owned help is kept beside its schema. Null defaults mean leave the member absent;
/// the image, host or resolved place settings supply it rather than an invented driver value.
fn field_help() -> Json {
    // Keep each declaration small enough for the macro's ordinary recursion limit.
    let groups = [
        json!({
            "image": {"title":"Image reference","description":"The image to pull from a registry. Choose a tag for a first template, or pin its digest for repeatable starts. Give exactly one image, build or archive.","default":null,"examples":["node:22-bookworm","ghcr.io/example/dev:1.0"]},
            "image_digest": {"title":"Pinned image digest","description":"Use a sha256 digest to keep a tag from changing the image on the next start. Leave unset to follow the image reference.","default":null,"examples":["sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]},
            "build": {"title":"Build from a Dockerfile","description":"Build an image on a host that offers builds. Use this instead of an image reference when the project needs extra tools.","default":null},
            "build.dockerfile": {"title":"Dockerfile","description":"The complete Dockerfile text. The host builds it before starting the unit; only classic Docker build syntax is supported.","default":null,"examples":["FROM node:22-bookworm\nRUN corepack enable\n"]},
            "build.context": {"title":"Build context","description":"Files available to COPY and ADD, from an uploaded archive or a public Git repository. Leave unset if the Dockerfile needs no other files.","default":null},
            "build.context.blob": {"title":"Context archive hash","description":"The 64 lowercase hexadecimal characters identifying an uploaded context archive. Upload the archive first.","default":null,"examples":["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]},
            "build.context.git": {"title":"Public Git repository","description":"A repository address for the build context, accessible without credentials. Choose a revision and directory too.","default":null,"examples":["https://example.org/team/project.git"]},
            "build.context.ref": {"title":"Git revision","description":"The branch, tag or commit to build. A commit makes the context repeatable.","default":null,"examples":["main","v1.0"]},
            "build.context.path": {"title":"Context directory","description":"The directory inside the repository used as the context. Use a dot for its root.","default":null,"examples":[".","tools/environment"]},
            "build.args": {"title":"Build arguments","description":"Plain build-time variables passed to ARG in the Dockerfile. Values are visible in the template; keep secrets out.","default":{}},
            "build.target": {"title":"Build stage","description":"The named stage to build in a multi-stage Dockerfile. Leave unset for the last stage.","default":null,"examples":["development"]},
            "archive": {"title":"Load an image archive","description":"Load a previously uploaded Docker or OCI image archive instead of pulling or building an image.","default":null},
            "archive.blob": {"title":"Image archive hash","description":"The 64 lowercase hexadecimal characters identifying an uploaded image archive.","default":null,"examples":["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]},
            "archive.image": {"title":"Image in the archive","description":"Choose the image by name if the archive contains several. Leave unset when it holds one image.","default":null,"examples":["team/dev:1.0"]},
        }),
        json!({
            "runtime": {"title":"Runtime","description":"The runtime an environment starts under, among those the chosen host reports. Whoever makes an environment can choose another unless the template limits the choice. Leave unset for the host's default; choose gVisor (runsc) or Kata only on a host that offers it.","default":null,"examples":["runc","runsc","kata"]},
            "allowed_runtimes": {"title":"Runtimes that may be chosen","description":"The runtimes whoever makes an environment from this template may choose, besides the one it starts under. Leave empty to permit every runtime the host offers.","default":[],"examples":[["runc","kata"]]},
            "image_facts": {"title":"What the image has","description":"What the named image is known to hold, as true or false for each of docker and qemu, read when it was built. They count only while the template's image is the one named. An environment asked to hold Docker inside says so when its image has no Docker. Leave empty for an image nobody checked.","default":{},"examples":[{"image":"team/dev:1.0","docker":true,"qemu":false}]},
            "image_facts.image": {"title":"Image the facts are about","description":"The image reference the facts below were read from. They count only while the template's image is this one.","default":null,"examples":["team/dev:1.0"]},
            "image_facts.docker": {"title":"Has Docker","description":"Whether the image holds Docker Engine. False makes the request for Docker inside say that Docker is to be installed.","default":null,"examples":[true]},
            "image_facts.qemu": {"title":"Has QEMU","description":"Whether the image holds QEMU's system emulators.","default":null,"examples":[false]},
            "user": {"title":"User","description":"Who runs the connector and commands inside the unit. Leave unset for the image's user. Numeric IDs take precedence over a name.","default":null},
            "user.name": {"title":"User name","description":"A user already present in the image. Change this only when the image provides that account.","default":null,"examples":["node","agent"]},
            "user.uid": {"title":"User ID","description":"Numeric user ID inside the unit. Match the owner of writable files in the image; zero runs as root.","default":null,"maximum":u32::MAX,"examples":[1000]},
            "user.gid": {"title":"Group ID","description":"Numeric group ID inside the unit. When omitted with a user ID, it uses the same number as the user ID.","default":null,"maximum":u32::MAX,"examples":[1000]},
            "resources": {"title":"Limits","description":"The most a unit may use. Unset amounts come from the resolved place settings; the host reports which limits it enforces.","default":{}},
            "resources.cpu_millis": {"title":"Processor time","description":"Processor limit in thousandths of a core: 1000 is one core, 4000 is four. Raise it for parallel builds.","default":null,"x-unit":"millicores","examples":[4000]},
            "resources.memory_mb": {"title":"Memory","description":"Memory limit in MiB. Raise it for large builds or language servers; 1024 MiB is one GiB.","default":null,"x-unit":"MiB","examples":[4096]},
            "resources.memory_swap_mb": {"title":"Memory and swap","description":"Combined memory and swap limit in MiB. Set memory too; this amount must be at least the memory limit.","default":null,"x-unit":"MiB","examples":[8192]},
            "resources.pids": {"title":"Processes and threads","description":"Maximum number of processes and threads together. Raise it if builds reach the process limit.","default":null,"x-unit":"processes","examples":[4096]},
            "resources.disk_gb": {"title":"Disk","description":"Disk limit in GB. Enforcement requires storage quotas on the host; otherwise usage is reported.","default":null,"x-unit":"GB","examples":[50]},
            "resources.blkio_weight": {"title":"Disk priority","description":"Relative disk access weight, from 10 to 1000; Docker defaults to 500. Higher values get more disk time under contention; host support is required.","default":500,"examples":[500]},
            "resources_max": {"title":"May be raised to","description":"The largest limits an environment manager may request. Every initial limit must fit within this ceiling.","default":{}},
            "needs": {"title":"Needs free","description":"Capacity a host must have free to be considered for placement. These amounts declare a need; they do not reserve resources or set limits.","default":{}},
        }),
        json!({
            "network": {"title":"Network","description":"How the unit reaches Orch and other services. A restricted network needs a host with verified proxy isolation.","default":{}},
            "network.mode": {"title":"Network connection","description":"Bridge permits network access under the egress policy. None reaches Orch alone through the host's proxy.","default":"bridge"},
            "network.egress": {"title":"Outgoing connections","description":"Open permits outbound access. Restricted reaches Orch and names in the place's network.allowed_hosts setting; configure that allow-list before starting.","default":"open"},
            "network.dns": {"title":"DNS servers","description":"IP addresses of name servers. Leave empty for network defaults; restricted and Orch-only networks use their proxy resolver and require this list to be empty.","default":[],"examples":[["1.1.1.1","2606:4700:4700::1111"]]},
            "security": {"title":"Confinement","description":"Container security options. Keep extra privileges absent unless the tools need them and the host policy allows them.","default":{}},
            "security.read_only_rootfs": {"title":"Read-only image files","description":"Prevent writes to image files. Workspace, home and explicitly writable mounts remain writable; add temporary storage if tools need it.","default":false},
            "security.no_new_privileges": {"title":"Prevent extra privileges","description":"Prevent processes gaining privileges after launch. Enable for images whose tools do not rely on setuid programs.","default":false},
            "security.cap_add": {"title":"Added kernel capabilities","description":"Linux capabilities added to the container. Leave empty unless a trusted tool requires one; host policy still applies.","default":[],"examples":[["SYS_PTRACE"]]},
            "security.cap_drop": {"title":"Removed kernel capabilities","description":"Linux capabilities removed from the container. ALL drops every optional capability; test that the chosen image works with it.","default":[],"examples":[["ALL"]]},
            "env": {"title":"Variables","description":"Plain variables inside the unit, stored visibly in the template. For credentials use the place's secret names instead. Orch's launch variables cannot be replaced.","default":{},"examples":[{"NODE_ENV":"development"}]},
            "mounts": {"title":"Extra storage","description":"Volumes, temporary memory storage or host directories attached in addition to managed state. Targets must not overlap one another or Orch's directories.","default":[]},
            "mounts.*.type": {"title":"Storage kind","description":"Volume keeps data, tmpfs holds temporary data in memory, and bind uses a host directory. A bind needs a source; tmpfs has no source.","default":null},
            "mounts.*.source": {"title":"Storage source","description":"A Docker volume name or absolute host directory. Leave unset for a new volume or for tmpfs.","default":null,"examples":["build-cache","/srv/team/data"]},
            "mounts.*.target": {"title":"Path inside the unit","description":"An absolute destination path outside workspace, home and Orch's reserved directories. Tools see the attached files here.","default":null,"pattern":"^/","examples":["/tmp","/opt/cache"]},
            "mounts.*.read_only": {"title":"Read-only storage","description":"Permit reads but prevent writes through this attachment. Enable when tools need reference files only.","default":false},
            "host_labels": {"title":"Required host labels","description":"A host must have every label in this list. Use labels to select machines with suitable devices or installed tools.","default":[],"examples":[["kvm"]]},
            "workspace_mount": {"title":"Workspace path","description":"Absolute path of the environment's persistent workspace volume. Match paths used by project commands; provider runtimes have no workspace.","default":"/workspace","pattern":"^/","examples":["/workspace"]},
            "home_mount": {"title":"Home path","description":"Absolute path of the environment's persistent home volume. Match the image user's home to keep tools and caches across starts.","default":"/home/agent","pattern":"^/","examples":["/home/node"]},
        }),
        json!({
            "base_capabilities": {"title":"Capabilities enabled initially","description":"Capabilities requested when the environment is made. Host and place approval still apply; KVM requires a host with the device.","default":[]},
            "allowed_capabilities": {"title":"Capabilities that may be requested","description":"Capabilities an environment manager may request later. Each remains subject to host policy and approval.","default":[]},
            "sandbox": {"title":"Tool confinement","description":"Default and maximum file access for the agent's tools within the container. This is separate from the container's host isolation.","default":{}},
            "sandbox.default": {"title":"Initial tool access","description":"Read only prevents writes, workspace permits writes in the workspace, and open uses the container boundary. The adapter must support the chosen mode.","default":null},
            "sandbox.max": {"title":"Widest tool access","description":"The widest tool access this template permits. Choose a ceiling that allows the initial tool access.","default":null},
            "agent_defaults": {"title":"Agent defaults","description":"Settings supplied to agents using this environment, such as approval.mode. Place settings can override them; unfamiliar settings can be edited as JSON.","default":{},"examples":[{"approval.mode":"on_request"}]},
            "exec": {"title":"Exec node","description":"Tools this environment offers to a harness running elsewhere. List installed executors in preference order.","default":{}},
            "exec.offers": {"title":"Offered executors","description":"Executor names in the order they are tried. Choose only tools installed in the image or supplied by the host.","default":[],"examples":[["codex-exec-server","claude-code","plain"]]},
            "exec.orch_tools": {"title":"Offer Orch tools","description":"Let the agent use Orch's shared tools when its adapter supports them. Disable to keep those tools out of the session.","default":true},
            "colocated_harnesses": {"title":"Harnesses available here","description":"Installed harnesses that may run beside tools in this environment. The actual arrangement also needs the provider owner's consent.","default":[],"examples":[["codex","claude-code"]]},
            "mcp_server_ids": {"title":"Tool server presets","description":"IDs of existing MCP server presets enabled for a new environment. Use presets you have permission to use.","default":[],"examples":[["00000000-0000-4000-8000-000000000001"]]},
            "after_up": {"title":"Commands after start","description":"Named commands run as the unit user before it becomes ready, on every new start. Use repeatable commands; a failure fails the start. Arguments are literal, without an implicit shell.","default":[]},
            "after_up.*.name": {"title":"Command name","description":"A unique name for this command. Commands run in name order; use a number prefix when order matters.","default":null,"examples":["10-cache"]},
            "after_up.*.command": {"title":"Program and arguments","description":"An executable installed in the image followed by literal arguments. Name a shell explicitly only when shell syntax is needed.","default":null,"examples":[["/usr/bin/mkdir","-p","/workspace/.cache"]]},
            "after_up.*.timeout_secs": {"title":"Time limit","description":"Maximum running time in seconds, from 1 to 3600. A timeout fails the start and stops the command's process group.","default":300,"x-unit":"seconds"},
            "init": {"title":"Earlier startup command","description":"An earlier command format, treated as the named command init. Prefer Commands after start for new templates.","default":null},
            "plugins": {"title":"Registered programs","description":"Programs installed in a provider image that supply provider details, translation, harness adapters or executors. They run beside the provider credential.","default":[]},
            "plugins.*.kind": {"title":"Program kind","description":"Which connector interface this program implements. Choose the interface declared by the program.","default":null},
            "plugins.*.name": {"title":"Registered name","description":"A unique program name that does not replace a built-in implementation. Other configuration selects the program by this name.","default":null,"examples":["team-provider"]},
            "plugins.*.command": {"title":"Program and arguments","description":"The program's absolute path in the image followed by its literal arguments. The program must implement the plugin transport.","default":null,"examples":[["/opt/team/provider","--stdio"]]},
            "plugins.*.env": {"title":"Program variables","description":"Additional plain variables for this program. It also inherits the unit's variables; keep secrets out of this visible map.","default":{}},
            "plugins.*.restart": {"title":"Restart policy","description":"On failure restarts a failed program, always also restarts after a clean exit, and never leaves an exited program stopped.","default":"on-failure"}
        }),
    ];
    Json::Object(
        groups
            .into_iter()
            .filter_map(|group| group.as_object().cloned())
            .flatten()
            .collect(),
    )
}

fn describe_fields(schema: &mut Json, path: &str, help: &Json) {
    let lookup = if let Some(suffix) = path
        .strip_prefix("resources_max.")
        .or_else(|| path.strip_prefix("needs."))
    {
        format!("resources.{suffix}")
    } else {
        path.to_owned()
    };
    if let Some(metadata) = help.get(&lookup).and_then(Json::as_object)
        && let Some(object) = schema.as_object_mut()
    {
        for (key, value) in metadata {
            object.insert(key.clone(), value.clone());
        }
    }
    if path.starts_with("needs.") && schema.get("description").is_some() {
        schema["description"] = json!(format!(
            "Declared capacity needed for placement, in {}. This does not reserve resources or set a usage limit.",
            schema
                .get("x-unit")
                .and_then(Json::as_str)
                .unwrap_or("the stated units")
        ));
        schema["default"] = Json::Null;
    }
    if path.starts_with("resources_max.") && schema.get("description").is_some() {
        schema["description"] = json!(format!(
            "Largest limit a manager may request, in {}. Leave unset for no template ceiling.",
            schema
                .get("x-unit")
                .and_then(Json::as_str)
                .unwrap_or("relative weight")
        ));
        schema["default"] = Json::Null;
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Json::as_object_mut) {
        for (key, field) in properties {
            describe_fields(
                field,
                &if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                },
                help,
            );
        }
    }
    if let Some(items) = schema.get_mut("items") {
        describe_fields(items, &format!("{path}.*"), help);
    }
    for union in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = schema.get_mut(union).and_then(Json::as_array_mut) {
            for variant in variants {
                describe_fields(variant, path, help);
            }
        }
    }
}

/// Complete starting configurations, shipped as schema annotations so every plugin driver can offer its own.
fn examples(kind: RuntimeKind) -> Json {
    let environment = kind == RuntimeKind::Environment;
    let base = |image: &str| {
        let mut spec = json!({"image":image,"network":{"mode":"bridge","egress":"open","dns":[]},
            "security":{"no_new_privileges":true},"resources":{"cpu_millis":4000,"memory_mb":4096,"pids":4096},
            "env":{},"mounts":[],"host_labels":[],"after_up":[]});
        if environment {
            spec["workspace_mount"] = json!("/workspace");
            spec["home_mount"] = json!("/home/agent");
            spec["exec"] = json!({"offers":["plain"],"orch_tools":true});
        }
        spec
    };
    let mut python = base("python:3.13-bookworm");
    python["env"] = json!({"DATABASE_URL":"postgresql://db.example.internal:5432/app"});
    let mut kvm = base("debian:bookworm");
    kvm["host_labels"] = json!(["kvm"]);
    if environment {
        kvm["base_capabilities"] = json!(["kvm"]);
        kvm["allowed_capabilities"] = json!(["kvm"]);
    }
    let mut restricted = base("node:22-bookworm");
    restricted["network"]["egress"] = json!("restricted");
    json!([
        {"id":"node","name":"Node","title":"Node web project","description":"A Node web project with four cores and 4 GiB of memory; install project dependencies in its workspace.","spec":base("node:22-bookworm")},
        {"id":"python-db","name":"Python","title":"Python with a database beside it","description":"Python connecting to a separately managed PostgreSQL service. Replace the example database address; select credentials through secret names.","spec":python},
        {"id":"rust","name":"Rust","title":"Rust project","description":"Rust with Cargo and the compiler in the image, ready for a workspace of Rust sources.","spec":base("rust:1-bookworm")},
        {"id":"kvm","name":"KVM","title":"Machine with KVM","description":if environment {"Select a host labelled kvm and request its KVM capability; host approval and /dev/kvm are required."} else {"Select a host labelled kvm for this runtime; a host label alone does not grant a device to a provider."},"spec":kvm},
        {"id":"restricted","name":"Restricted","title":"Restricted outgoing connections","description":"Reach Orch and the host names in the place's network.allowed_hosts allow-list. Set that list before starting; the host must support restricted egress.","spec":restricted}
    ])
}

/// The schema of the spec of a template for providers' runtimes.
#[must_use]
pub fn provider_schema() -> Json {
    let mut schema = schema(
        "A Docker container for a provider's runtime",
        vec![
            (
                "plugins",
                orch_settings_contract::unit::plugins_template_schema(),
            ),
            (
                "needs",
                resources_schema("What this runtime declares it needs at least"),
            ),
        ],
    );
    schema["x-orch-examples"] = examples(RuntimeKind::Provider);
    schema
}

/// What a person gives for each of the imports the driver declares.
#[must_use]
pub fn imports() -> Vec<ImportDef> {
    let optional = |mut field: Field| {
        field.required = false;
        field
    };
    let mut dockerfile = Field::new("dockerfile", "The Dockerfile", FieldType::SecretText);
    dockerfile.max_bytes = Some(DOCKERFILE_MAX_BYTES as u64);
    dockerfile.help = Some("The text of the Dockerfile, pasted or chosen as a file.".to_owned());
    let mut context = Field::new("context_blob", "A context archive", FieldType::Text);
    context.help = Some(
        "The hash of an uploaded tar, tar.gz or zip archive with the files the Dockerfile needs."
            .to_owned(),
    );
    let mut git = Field::new("git", "A Git address for its context", FieldType::Text);
    git.help =
        Some("A repository that needs no credential; give this or a context archive.".to_owned());
    let mut git_ref = Field::new("git_ref", "The branch, tag or commit", FieldType::Text);
    git_ref.default = Some(json!("main"));
    let mut git_path = Field::new(
        "git_path",
        "The directory in the repository",
        FieldType::Text,
    );
    git_path.default = Some(json!("."));
    let mut reference = Field::new("reference", "An image reference", FieldType::Text);
    reference.help = Some("A registry reference, such as ghcr.io/org/image:1.2.3.".to_owned());
    let mut blob = Field::new("blob", "An image archive", FieldType::Text);
    blob.help = Some("The hash of an uploaded image archive: what `docker save` writes, or an OCI layout as a tar file.".to_owned());
    let mut which = Field::new("image", "Which image of the archive", FieldType::Text);
    which.help = Some("The name of one image, when the archive holds several.".to_owned());
    vec![
        ImportDef {
            format: IMPORT_DOCKERFILE.to_owned(),
            title: "A Dockerfile".to_owned(),
            fields: vec![
                dockerfile,
                optional(context),
                optional(git),
                optional(git_ref),
                optional(git_path),
            ],
        },
        ImportDef {
            format: IMPORT_IMAGE.to_owned(),
            title: "An image from a registry".to_owned(),
            fields: vec![reference],
        },
        ImportDef {
            format: IMPORT_ARCHIVE.to_owned(),
            title: "An image archive".to_owned(),
            fields: vec![blob, optional(which)],
        },
    ]
}

/// The sources this host offers: a registry reference and an archive always, and a Dockerfile unless the operator
/// turned building off.
#[must_use]
pub fn sources(config: &DockerConfig) -> Vec<SourceKind> {
    let mut sources = vec![SourceKind::Image];
    if config.builds {
        sources.push(SourceKind::Build);
    }
    sources.push(SourceKind::Archive);
    sources
}

/// The declaration of the driver for the daemon it is connected to.
#[must_use]
pub fn descriptor(facts: &DaemonFacts, config: &DockerConfig) -> DriverDescriptor {
    let mut descriptor = DriverDescriptor {
        name: NAME.to_owned(),
        version: VERSION.to_owned(),
        title: TITLE.to_owned(),
        configuration_schema: None,
        host_scope: Some(orch_driver_contract::host::HostScope::Node),
        tool_delivery: Some(orch_driver_contract::host::ToolDelivery::HostDirectory),
        gateway_access: Some(orch_driver_contract::host::GatewayAccess::NodeAddress),
        volume_export_consistency: None,
        host_ports: orch_driver_contract::descriptor::OperationSupport::available(true, ""),
        kept_file_transfer: orch_driver_contract::descriptor::OperationSupport::available(true, ""),
        local_accounts: false,
        unavailable: BTreeMap::new(),
        spec_schemas: BTreeMap::from([
            (RuntimeKind::Environment, environment_schema()),
            (RuntimeKind::Provider, provider_schema()),
        ]),
        confinement: Confinement::Isolated,
        confinement_by: None,
        lifecycle: orch_driver_contract::descriptor::Lifecycle::Unit,
        commands_after_start:
            orch_driver_contract::descriptor::OperationSupport::process_group_commands(true),
        // Open always; restricted only where this daemon was found, by trying, to make a unit's own isolated
        // network for it and its proxy.
        network_egress: if facts.restricted_egress
            && !facts.rootless
            && config.egress.proxy_image.is_some()
        {
            vec![EgressMode::Open, EgressMode::Restricted]
        } else {
            vec![EgressMode::Open]
        },
        capabilities: capabilities::declared(),
        recreate: Recreate::Replace,
        live_changes: orch_driver_contract::descriptor::LiveChanges {
            gateway_given: orch_driver_contract::capabilities::GATEWAY_GIVEN
                .iter()
                .map(|key| (*key).into())
                .collect(),
            resources: facts.limits_enforced(),
            mounts: false,
            capabilities: Vec::new(),
        },
        recreate_preserves_files: true,
        volumes: true,
        data_storage: orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Volume),
            existing_directory: cfg!(target_os = "linux")
                && !facts.rootless
                && config.live_helper_binary.is_some(),
            reason: (!(cfg!(target_os = "linux")
                && !facts.rootless
                && config.live_helper_binary.is_some()))
            .then(|| {
                "existing directories require a Linux daemon and the node's trusted static helper"
                    .into()
            }),
        },
        mounts: MountPlacement::Placed,
        private_views: facts.private_views(),
        storages: true,
        suspend: facts.can_pause(),
        sources: sources(config),
        imports: imports(),
        // Memory, processor time and processes are limits the daemon enforces when the kernel can; the disk is
        // measured and enforced only where the storage driver has quotas, which the template's limits say.
        resource_limits: orch_driver_contract::descriptor::ResourceLimitSupport {
            reasons: Default::default(),
            cpu: if facts.cpu_quota {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            memory: if facts.memory_limit {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            pids: if facts.pids_limit {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            disk: if facts.disk_quota {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
        },
        process_scope: ProcessScope::All,
        runtime_classes: (!facts.runtimes.is_empty()).then(|| facts.runtimes.clone()),
        runtimes: Vec::new(),
        default_runtime: None,
        extensions: Default::default(),
    };
    // What the interface may show of the daemon; the core never reads it.
    let daemon = json!({
        "version": facts.server_version, "api_version": facts.api_version,
        "storage_driver": facts.storage_driver, "cgroup_version": facts.cgroup_version,
        "rootless": facts.rootless, "default_runtime": facts.default_runtime,
        "disk_quota": facts.disk_quota,
    });
    if descriptor.extensions.insert("x_daemon", daemon).is_err() {
        tracing::debug!("the descriptor could not take the facts of the daemon");
    }
    descriptor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_docker_enum_choice_has_words_in_both_catalogues() {
        fn choices(schema: &Json, path: &str, catalogue: &Json) {
            if let Some(values) = schema.get("enum").and_then(Json::as_array) {
                for value in values {
                    let pointer = format!(
                        "/templates/choices/{}/{}",
                        path.replace('.', "/"),
                        value.as_str().unwrap()
                    );
                    for member in ["name", "description"] {
                        let field = format!("{pointer}/{member}");
                        assert!(
                            catalogue
                                .pointer(&field)
                                .and_then(Json::as_str)
                                .is_some_and(|word| !word.is_empty()),
                            "{field}"
                        );
                    }
                }
            }
            if let Some(properties) = schema.get("properties").and_then(Json::as_object) {
                for (key, field) in properties {
                    choices(
                        field,
                        &if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        },
                        catalogue,
                    );
                }
            }
            if let Some(items) = schema.get("items") {
                choices(items, &format!("{path}.*"), catalogue);
            }
            for union in ["anyOf", "oneOf", "allOf"] {
                if let Some(variants) = schema.get(union).and_then(Json::as_array) {
                    for variant in variants {
                        choices(variant, path, catalogue);
                    }
                }
            }
        }
        for text in [
            include_str!("../../../../frontend/src/i18n/en.json"),
            include_str!("../../../../frontend/src/i18n/zh-CN.json"),
        ] {
            let catalogue: Json = serde_json::from_str(text).unwrap();
            for schema in [environment_schema(), provider_schema()] {
                choices(&schema, "", &catalogue);
            }
        }
    }

    fn facts() -> DaemonFacts {
        DaemonFacts {
            os_type: "linux".into(),
            cgroup_version: 2,
            memory_limit: true,
            cpu_quota: true,
            pids_limit: true,
            kernel_overlay: true,
            runtimes: vec!["io.containerd.runc.v2".into(), "runc".into()],
            ..DaemonFacts::default()
        }
    }

    #[test]
    fn the_descriptor_is_a_driver_that_holds_together_and_says_what_this_daemon_can_do() {
        let descriptor = descriptor(&facts(), &DockerConfig::default());
        assert_eq!(descriptor.check(), Ok(()));
        assert_eq!(descriptor.confinement, Confinement::Isolated);
        assert_eq!(descriptor.recreate, Recreate::Replace);
        assert_eq!(descriptor.mounts, MountPlacement::Placed);
        assert!(
            descriptor.volumes
                && descriptor.storages
                && descriptor.private_views
                && descriptor.suspend
        );
        assert_eq!(descriptor.resource_limits.cpu, ResourceLimits::Enforced);
        assert_eq!(descriptor.resource_limits.memory, ResourceLimits::Enforced);
        assert_eq!(descriptor.resource_limits.pids, ResourceLimits::Enforced);
        assert_eq!(
            descriptor.resource_limits.disk,
            ResourceLimits::Reported,
            "process controllers do not imply a disk quota"
        );
        assert_eq!(descriptor.process_scope, ProcessScope::All);
        assert!(
            descriptor.makes_units_for(RuntimeKind::Environment)
                && descriptor.makes_units_for(RuntimeKind::Provider)
        );
        // The eight the driver always had, and Docker inside.
        assert_eq!(descriptor.capabilities.len(), 9);
        assert_eq!(
            descriptor.runtime_classes.as_deref(),
            Some(&["io.containerd.runc.v2".to_owned(), "runc".to_owned()][..])
        );
        assert!(
            descriptor.accepts_runtime_class(Some("runc"))
                && !descriptor.accepts_runtime_class(Some("kata"))
        );
        for format in ["dockerfile", "image", "archive"] {
            assert!(descriptor.import(format).is_some(), "{format}");
        }
    }

    #[test]
    fn what_the_daemon_cannot_do_is_not_claimed() {
        let weak = DaemonFacts {
            os_type: "linux".into(),
            cgroup_version: 1,
            rootless: true,
            ..DaemonFacts::default()
        };
        let descriptor = descriptor(
            &weak,
            &DockerConfig {
                builds: false,
                ..DockerConfig::default()
            },
        );
        assert_eq!(descriptor.check(), Ok(()));
        assert!(!descriptor.private_views && !descriptor.suspend);
        assert_eq!(descriptor.resource_limits.cpu, ResourceLimits::Reported);
        assert_eq!(
            descriptor.runtime_classes, None,
            "no classes are listed when the daemon names none"
        );
        assert!(
            !descriptor.has_source(SourceKind::Build),
            "a host that does not build does not offer it"
        );
        assert!(
            descriptor.has_source(SourceKind::Image) && descriptor.has_source(SourceKind::Archive)
        );
    }

    #[test]
    fn the_descriptor_survives_its_own_wire_form() {
        let original = descriptor(&facts(), &DockerConfig::default());
        let wire = serde_json::to_value(&original).unwrap();
        let read: DriverDescriptor = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(read, original);
        assert_eq!(wire["x_daemon"]["rootless"], false);
    }

    #[test]
    fn the_schema_of_each_pool_is_closed_and_has_the_members_that_belong_to_that_pool() {
        for schema in [environment_schema(), provider_schema()] {
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema["properties"]["image"].is_object());
            assert!(schema["properties"]["security"]["properties"]["read_only_rootfs"].is_object());
        }
        assert!(environment_schema()["properties"]["workspace_mount"].is_object());
        assert!(
            provider_schema()["properties"]["workspace_mount"].is_null(),
            "a provider's runtime has no workspace"
        );
        assert!(
            environment_schema()["properties"]["allowed_capabilities"]["items"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("kvm"))
        );
        for field in ["base_capabilities", "allowed_capabilities"] {
            let schema = environment_schema();
            let keys = schema["properties"][field]["items"]["enum"]
                .as_array()
                .unwrap();
            for key in orch_driver_contract::capabilities::GATEWAY_GIVEN {
                assert!(keys.contains(&json!(key)), "{field}: {key}");
            }
            assert!(!keys.contains(&json!("unknown")));
        }
    }

    #[test]
    fn each_import_asks_for_what_it_needs_and_checks_what_it_is_given() {
        let imports = imports();
        let dockerfile = imports
            .iter()
            .find(|import| import.format == "dockerfile")
            .unwrap();
        assert_eq!(
            dockerfile.check_values(&json!({ "dockerfile": "FROM x" })),
            Ok(())
        );
        assert!(dockerfile.check_values(&json!({})).is_err());
        assert!(
            dockerfile
                .check_values(&json!({ "dockerfile": "FROM x", "typo": 1 }))
                .is_err()
        );
        let image = imports
            .iter()
            .find(|import| import.format == "image")
            .unwrap();
        assert_eq!(
            image.check_values(&json!({ "reference": "busybox" })),
            Ok(())
        );
        let archive = imports
            .iter()
            .find(|import| import.format == "archive")
            .unwrap();
        assert!(
            archive
                .check_values(&json!({ "blob": "0".repeat(64), "image": "x:1" }))
                .is_ok()
        );
    }

    #[test]
    fn a_new_writer_gets_explained_fields_and_working_examples_for_both_unit_kinds() {
        fn explained(schema: &Json) {
            if let Some(properties) = schema.get("properties").and_then(Json::as_object) {
                for (name, field) in properties {
                    assert!(
                        field["title"].as_str().is_some_and(|text| !text.is_empty()),
                        "{name}: title"
                    );
                    assert!(
                        field["description"]
                            .as_str()
                            .is_some_and(|text| !text.is_empty()),
                        "{name}: description"
                    );
                    assert!(field.get("default").is_some(), "{name}: default");
                    explained(field);
                }
            }
            if let Some(items) = schema.get("items") {
                explained(items);
            }
            for union in ["anyOf", "oneOf"] {
                if let Some(variants) = schema.get(union).and_then(Json::as_array) {
                    for variant in variants {
                        explained(variant);
                    }
                }
            }
        }
        for (kind, schema) in [
            (RuntimeKind::Environment, environment_schema()),
            (RuntimeKind::Provider, provider_schema()),
        ] {
            explained(&schema);
            let validator = jsonschema::validator_for(&schema).unwrap();
            for example in schema["x-orch-examples"].as_array().unwrap() {
                let spec = &example["spec"];
                assert!(
                    validator.is_valid(spec),
                    "{}: {:?}",
                    example["title"],
                    validator
                        .iter_errors(spec)
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                );
                assert!(
                    crate::spec::DockerSpec::parse(kind, spec).is_ok(),
                    "{}",
                    example["title"]
                );
                assert!(
                    orch_settings_contract::unit::template_plugins(
                        spec,
                        kind == RuntimeKind::Provider
                    )
                    .is_ok()
                );
            }
        }
    }
}
