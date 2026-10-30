//! What an environment runs on and under: the runtime it is made under, what its host declares for it, the policy of its place, and the lines people and agents are told.
//!
//! The runtime of an environment is chosen when it is made (`env.runtime`), among those its host reports; the template gives
//! the default (its `runtime`) and may limit the choice (`allowed_runtimes`), and a place's policy (`environments.runtimes`)
//! may forbid runtimes or require one. A unit made under a runtime has the driver's declaration laid over by the runtime's
//! class ([`orch_driver_contract::isolation`]), so that what the core reads of the host (the live changes it makes, the
//! capabilities it gives, whether files stay across a replacement) holds for that unit. Nothing here tests a runtime's name:
//! every difference is data the driver declared.

use orch_driver_contract::isolation::{
    Boundary, RuntimeClass, RuntimeIsolation, RuntimeLimit, SUBJECT_MOUNTS, effective_json,
};
use orch_types::Json;
use std::collections::BTreeMap;

/// The runtime the recorded definition of a unit names: the body of the command that made it holds the template's spec.
#[must_use]
pub fn definition_runtime(definition: Option<&Json>) -> Option<&str> {
    definition?.get("spec")?.get("runtime")?.as_str()
}

/// The driver's declaration for the unit whose recorded definition is `definition`, on a host that reports `drivers`
/// (by driver name): the descriptor laid over by the unit's runtime. None where the host does not report the driver.
#[must_use]
pub fn unit_driver(drivers: &Json, driver: &str, definition: Option<&Json>) -> Option<Json> {
    Some(effective_json(
        drivers.get(driver)?,
        definition_runtime(definition),
    ))
}

/// Class limitations that explicitly name a subject for a particular unit.
/// An empty `touches` list describes the runtime generally and does not by
/// itself mean that a specific operation must be refused.
#[must_use]
pub fn explicit_subject_limits(
    drivers: &Json,
    driver: &str,
    definition: Option<&Json>,
    subject: &str,
) -> Vec<RuntimeLimit> {
    unit_driver(drivers, driver, definition)
        .and_then(|descriptor| orch_driver_contract::isolation::class_in(&descriptor, None))
        .map(|class| {
            class
                .limits
                .into_iter()
                .filter(|limit| limit.touches.iter().any(|touched| touched == subject))
                .collect()
        })
        .unwrap_or_default()
}

/// The runtime an environment was given when it was made (or later, through its settings): `env.runtime` as its place
/// resolves it, none where it has taken its template's own.
pub async fn chosen_runtime(
    conn: &mut orch_db::Conn,
    environment: orch_types::Id,
) -> Result<Option<String>, crate::error::EngineError> {
    Ok(orch_settings::get::<String>(
        conn,
        orch_settings::Place::Environment(environment),
        orch_settings_contract::registry::keys::ENV_RUNTIME,
    )
    .await?
    .filter(|runtime| !runtime.is_empty()))
}

/// How long the connector of a unit made under the runtime its recorded spec names has to enroll: the longer of the
/// installation's figure and what the runtime's class declares, as a unit of a virtual machine boots before it can.
#[must_use]
pub fn enroll_time(
    ordinary: std::time::Duration,
    drivers: &Json,
    driver: &str,
    spec: &Json,
) -> std::time::Duration {
    let declared = drivers
        .get(driver)
        .and_then(|descriptor| {
            orch_driver_contract::isolation::class_in(
                descriptor,
                spec.get("runtime").and_then(Json::as_str),
            )
        })
        .and_then(|class| class.enrol_timeout_secs)
        .map(std::time::Duration::from_secs);
    declared.map_or(ordinary, |declared| declared.max(ordinary))
}

/// What a place's policy `environments.runtimes` says of a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// It may be chosen.
    Allow,
    /// It may not.
    Forbid,
    /// It is the one that may be chosen, with the others that are required.
    Require,
}

/// The rule for `runtime` in a policy map: its own entry, else the entry `*`, else allow.
fn rule_of(rules: &BTreeMap<String, String>, runtime: &str) -> Rule {
    let word = rules.get(runtime).or_else(|| rules.get("*"));
    match word.map(String::as_str) {
        Some("forbid") => Rule::Forbid,
        Some("require") => Rule::Require,
        _ => Rule::Allow,
    }
}

/// Why `runtime` may not be chosen where `policy` (the resolved `environments.runtimes`) holds, if it may not: it is
/// forbidden, or some other runtime is required and this one is not named required.
pub fn check_policy(policy: &Json, runtime: &str) -> Result<(), String> {
    let rules: BTreeMap<String, String> = policy
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(name, rule)| {
                    rule.as_str().map(|rule| (name.clone(), rule.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    if rule_of(&rules, runtime) == Rule::Forbid {
        return Err(format!("the runtime {runtime} is not allowed here"));
    }
    let required: Vec<&str> = rules
        .iter()
        .filter(|(name, rule)| rule.as_str() == "require" && name.as_str() != "*")
        .map(|(name, _)| name.as_str())
        .collect();
    if !required.is_empty() && rule_of(&rules, runtime) != Rule::Require {
        return Err(format!(
            "environments here are made under {}",
            required.join(" or ")
        ));
    }
    Ok(())
}

/// Checks the actual host class, including its default and aliases, against the place's policy.
/// An alias cannot make a forbidden class available; conflicting alias rules keep the stricter rule.
pub fn check_host_policy(
    policy: &Json,
    descriptor: &Json,
    requested: Option<&str>,
) -> Result<(), String> {
    let Some(class) = orch_driver_contract::isolation::class_in(descriptor, requested) else {
        if let Some(runtime) =
            requested.or_else(|| descriptor.get("default_runtime").and_then(Json::as_str))
        {
            return check_policy(policy, runtime);
        }
        // An older node cannot prove that an unnamed default meets a restrictive policy.
        let restrictive = policy.as_object().is_some_and(|rules| {
            rules
                .values()
                .any(|rule| matches!(rule.as_str(), Some("forbid" | "require")))
        });
        return if restrictive {
            Err("the host must report its default runtime before this runtime policy can be checked".to_owned())
        } else {
            Ok(())
        };
    };
    let mut rules = policy.as_object().cloned().unwrap_or_default();
    let priority = |rule: &Json| match rule.as_str() {
        Some("forbid") => 3,
        Some("require") => 2,
        Some("allow") => 1,
        _ => 0,
    };
    let mut strongest: Option<Json> = None;
    for name in std::iter::once(&class.name).chain(class.aliases.iter()) {
        if let Some(rule) = rules.remove(name)
            && strongest
                .as_ref()
                .is_none_or(|held| priority(&rule) > priority(held))
        {
            strongest = Some(rule);
        }
    }
    if let Some(rule) = strongest {
        rules.insert(class.name.clone(), rule);
    }
    check_policy(&Json::Object(rules), &class.name)
}

/// Why a template does not let `runtime` be chosen: its `allowed_runtimes` does not list it. A template that lists none
/// permits every runtime, and the runtime it names itself is always permitted.
pub fn check_template(spec: &Json, runtime: &str) -> Result<(), String> {
    let allowed = spec.get("allowed_runtimes").and_then(Json::as_array);
    let own = spec.get("runtime").and_then(Json::as_str);
    match allowed {
        Some(list) if !list.is_empty() && own != Some(runtime) => {
            if list.iter().any(|name| name.as_str() == Some(runtime)) {
                Ok(())
            } else {
                Err(format!(
                    "this template does not permit the runtime {runtime}"
                ))
            }
        }
        _ => Ok(()),
    }
}

/// The spec of a unit with the runtime an environment chose written over the template's own default.
pub fn apply_choice(spec: &mut Json, runtime: Option<&str>) {
    if let (Some(runtime), Some(members)) = (runtime, spec.as_object_mut()) {
        members.insert("runtime".to_owned(), Json::String(runtime.to_owned()));
    }
}

/// The words of what holds a unit in, as an agent is told them.
#[must_use]
pub const fn boundary_hint(boundary: Boundary) -> &'static str {
    match boundary {
        Boundary::Unbounded => "the machine itself",
        Boundary::SharedKernel => "shares the host's kernel",
        Boundary::UserSpaceKernel => "its own kernel written in user space",
        Boundary::OwnKernel => "a microVM with its own kernel",
    }
}

/// What a runtime is, with the limits that touch the environment: `subjects` are the capability keys it holds, is offered or
/// allows, and the word of what it has attached.
#[must_use]
pub fn isolation_of(
    driver: &str,
    driver_title: &str,
    class: Option<&RuntimeClass>,
    runtime: Option<&str>,
    fallback: Boundary,
    subjects: &[&str],
) -> RuntimeIsolation {
    let limits: Vec<RuntimeLimit> = class
        .map(|class| {
            class
                .limits_touching(subjects)
                .into_iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let unavailable = class
        .map(|class| {
            class
                .unavailable
                .iter()
                .filter(|(key, _)| subjects.contains(&key.as_str()))
                .map(|(key, why)| (key.clone(), why.clone()))
                .collect()
        })
        .unwrap_or_default();
    RuntimeIsolation {
        driver: driver.to_owned(),
        driver_title: driver_title.to_owned(),
        runtime: class
            .map(|class| class.name.clone())
            .or_else(|| runtime.map(str::to_owned)),
        runtime_title: class
            .map(|class| class.title.clone())
            .or_else(|| runtime.map(str::to_owned)),
        boundary: class.map_or(fallback, |class| class.boundary),
        keeps_software_on_stop: class.map(|class| class.keeps_software_on_stop),
        limits,
        unavailable,
    }
}

/// What an environment runs on and under, from the descriptor its host reports for it (laid over by its runtime):
/// `subjects` are what it has, is offered or allows.
#[must_use]
pub fn isolation_from_descriptor(
    driver: &str,
    descriptor: &Json,
    subjects: &[&str],
) -> RuntimeIsolation {
    let class = orch_driver_contract::isolation::class_in(descriptor, None);
    let title = descriptor
        .get("title")
        .and_then(Json::as_str)
        .unwrap_or(driver);
    // A driver that declares no classes holds a unit in as it declares its confinement does.
    let fallback = match descriptor.get("confinement").and_then(Json::as_str) {
        Some("isolated") => Boundary::SharedKernel,
        _ => Boundary::Unbounded,
    };
    isolation_of(driver, title, class.as_ref(), None, fallback, subjects)
}

/// What a runtime's class says of one capability: its risk there (the definition's own where the class says nothing) and the
/// limits that name it, as sentences with their first letter capitalised.
#[must_use]
pub fn capability_words(descriptor: &Json, key: &str, defined_risk: &str) -> (String, Vec<String>) {
    let Some(class) = orch_driver_contract::isolation::class_in(descriptor, None) else {
        return (defined_risk.to_owned(), Vec::new());
    };
    let risk = class.risk_of(key, defined_risk).to_owned();
    let limits = class
        .limits
        .iter()
        .filter(|limit| limit.touches.iter().any(|touched| touched == key))
        .map(|limit| {
            let sentence = limit.sentence();
            let mut letters = sentence.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect();
    (risk, limits)
}

/// The line "where you are" tells an agent: the pair, what holds the environment in, and the limits that touch what it has
/// or can be given, each as what to do and why.
#[must_use]
pub fn agent_line(isolation: &RuntimeIsolation) -> String {
    let mut line = format!("- Runs on {}", isolation.driver_title.trim());
    if let Some(title) = &isolation.runtime_title {
        line.push_str(" · ");
        line.push_str(title);
    }
    line.push_str(&format!(" ({})", boundary_hint(isolation.boundary)));
    line.push('.');
    if !isolation.limits.is_empty() {
        line.push_str(" Here: ");
        let sentences: Vec<String> = isolation
            .limits
            .iter()
            .map(RuntimeLimit::sentence)
            .collect();
        let terminal = |text: &str| {
            text.trim_end()
                .chars()
                .last()
                .is_some_and(|character| matches!(character, '.' | '!' | '?'))
        };
        for (index, sentence) in sentences.iter().enumerate() {
            let text = sentence.trim();
            if index > 0 && terminal(&sentences[index - 1]) {
                let mut letters = text.chars();
                if let Some(first) = letters.next() {
                    line.extend(first.to_uppercase());
                    line.extend(letters);
                }
            } else {
                line.push_str(text);
            }
            match sentences.get(index + 1) {
                None if !terminal(text) => line.push('.'),
                Some(_) if terminal(text) => line.push(' '),
                Some(next) if terminal(next) => line.push_str(". "),
                Some(_) => line.push_str("; "),
                None => {}
            }
        }
    }
    line
}

/// The subjects a limit is matched against: the capability keys, and the feature word of attached mounts.
#[must_use]
pub fn subjects<'a>(keys: impl IntoIterator<Item = &'a str>, has_mounts: bool) -> Vec<&'a str> {
    let mut subjects: Vec<&str> = keys.into_iter().collect();
    if has_mounts {
        subjects.push(SUBJECT_MOUNTS);
    }
    subjects.sort_unstable();
    subjects.dedup();
    subjects
}

/// What a replacement tells an agent (change line) when it was made again: `reason` names why, `kept` whether the files
/// outside the volumes came across.
#[must_use]
pub fn replaced_line(reason: &str, kept: bool) -> String {
    if kept {
        format!(
            "- This environment was made again ({reason}): the software you installed outside the volumes is kept."
        )
    } else {
        format!(
            "- This environment was made again ({reason}): software you installed outside the volumes is gone; install it again, or use the Developer tools template."
        )
    }
}

/// Identifies the image source, including a build or archive, independently of runtime settings.
fn image_source_key(spec: &Json) -> String {
    let source = serde_json::json!({
        "image": spec.get("image"),
        "image_digest": spec.get("image_digest"),
        "build": spec.get("build"),
        "archive": spec.get("archive"),
    });
    orch_types::Hash::of(source.to_string().as_bytes()).to_string()
}

/// Observations returned by the node when it made an image available. An absent observation stays unknown;
/// it never falls back to a template author's assertion about installed programs.
#[must_use]
pub fn observed_image_facts(spec: &Json, programs: &BTreeMap<String, bool>) -> Json {
    let mut facts = serde_json::json!({ "source": image_source_key(spec) });
    for name in ["docker", "qemu"] {
        if let Some(present) = programs.get(name) {
            facts[name] = Json::Bool(*present);
        }
    }
    facts
}

/// Whether the recorded definition of a unit says that its image lacks `what` (`docker`, `qemu`). Observations
/// count only for that image source; older definitions may identify a reference by its image name alone.
#[must_use]
pub fn image_lacks(definition: Option<&Json>, what: &str) -> bool {
    let Some(spec) = definition.and_then(|definition| definition.get("spec")) else {
        return false;
    };
    let facts = spec.get("image_facts");
    let same_image = match facts.and_then(|facts| facts.get("source")) {
        Some(Json::String(source)) => *source == image_source_key(spec),
        Some(_) => false,
        None => facts
            .and_then(|facts| facts.get("image"))
            .and_then(Json::as_str)
            .is_some_and(|named| spec.get("image").and_then(Json::as_str) == Some(named)),
    };
    same_image
        && facts
            .and_then(|facts| facts.get(what))
            .and_then(Json::as_bool)
            == Some(false)
}

/// What the capability dialog says of Docker inside where the image has no Docker.
pub const DOCKER_MISSING_NOTE: &str = "Docker is not installed in this image: install it with sudo (it lasts until the environment is recreated), or make the environment from Developer tools, which has it built in.";

/// What the plans of a replacement and the capability dialog say where installed software is removed.
pub const REPLACED_PLAN_NOTE: &str = "Software installed since the environment was made, outside its volumes (for example with apt), is removed. The image's own and the volumes stay.";

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::isolation::RuntimeClass;
    use serde_json::json;

    #[test]
    fn operation_limits_come_from_the_recorded_class_and_only_its_explicit_subject() {
        let mut a = RuntimeClass::new("a", Boundary::SharedKernel);
        let mut b = RuntimeClass::new("b", Boundary::OwnKernel);
        a.limits.push(RuntimeLimit {
            key: "unrelated".into(),
            touches: vec![],
            todo: None,
            why: "a general hint".into(),
        });
        b.limits = vec![
            RuntimeLimit {
                key: "general".into(),
                touches: vec![],
                todo: None,
                why: "a general hint".into(),
            },
            RuntimeLimit {
                key: "carry".into(),
                touches: vec!["volume_carry".into()],
                todo: None,
                why: "Copy files first.".into(),
            },
        ];
        let drivers = json!({"other-driver": {"default_runtime":"a", "runtimes":[a,b]}});
        let made = json!({"spec":{"runtime":"b"}});
        let limits = explicit_subject_limits(&drivers, "other-driver", Some(&made), "volume_carry");
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].key, "carry");
        assert_eq!(limits[0].sentence(), "Copy files first.");
        assert!(explicit_subject_limits(&drivers, "other-driver", None, "volume_carry").is_empty());
        assert!(
            explicit_subject_limits(
                &json!({"old-driver":{}}),
                "old-driver",
                None,
                "volume_carry"
            )
            .is_empty()
        );
    }

    fn gvisor() -> RuntimeClass {
        let mut class = RuntimeClass::new("runsc", Boundary::UserSpaceKernel);
        class.title = "gVisor".into();
        class.limits = vec![
            RuntimeLimit {
                key: "live_changes".into(),
                touches: vec![],
                todo: Some("to change a mount, the environment restarts".into()),
                why: "its units cannot be reached from the host".into(),
            },
            RuntimeLimit {
                key: "docker_inside_ports".into(),
                touches: vec!["docker_inside".into()],
                todo: Some(
                    "run containers you start inside with --network=host to serve a port".into(),
                ),
                why: "-p and --expose do not work here".into(),
            },
        ];
        class
            .unavailable
            .insert("kvm".into(), "no hardware virtualisation to give".into());
        class
    }

    #[test]
    fn an_image_is_said_to_lack_docker_only_while_the_facts_name_the_image_the_unit_was_made_from()
    {
        let made =
            |image: &str, facts: Json| json!({ "spec": { "image": image, "image_facts": facts } });
        let lacking = made(
            "orch-env-base:1",
            json!({ "image": "orch-env-base:1", "docker": false }),
        );
        assert!(image_lacks(Some(&lacking), "docker"));
        assert!(
            !image_lacks(Some(&lacking), "qemu"),
            "nothing is said of what the facts do not state"
        );
        let has = made(
            "orch-env-dev:1",
            json!({ "image": "orch-env-dev:1", "docker": true }),
        );
        assert!(!image_lacks(Some(&has), "docker"));
        // A template whose image was changed after the facts were read says nothing of the new image.
        let changed = made(
            "team/mine:2",
            json!({ "image": "orch-env-base:1", "docker": false }),
        );
        assert!(!image_lacks(Some(&changed), "docker"));
        let unnamed = made("orch-env-base:1", json!({ "docker": false }));
        assert!(!image_lacks(Some(&unnamed), "docker"));
        assert!(!image_lacks(None, "docker"));
    }

    #[test]
    fn observed_programs_replace_declared_facts_and_follow_only_the_image_source() {
        let sources = [
            json!({"image": "team/image:1", "image_digest": "sha256:one"}),
            json!({"build": {"dockerfile": "FROM team/image:1"}}),
            json!({"archive": {"blob": "one", "image": "team/image:1"}}),
        ];
        for mut spec in sources {
            spec["image_facts"] = json!({"docker": true, "qemu": false});
            let observations = BTreeMap::from([("docker".to_owned(), false)]);
            spec["image_facts"] = observed_image_facts(&spec, &observations);
            let mut made = json!({"spec": spec});
            assert!(image_lacks(Some(&made), "docker"));
            assert!(
                !image_lacks(Some(&made), "qemu"),
                "unobserved stays unknown"
            );
            made["spec"]["runtime"] = json!("another-runtime");
            assert!(
                image_lacks(Some(&made), "docker"),
                "a runtime is not an image source"
            );
            for field in ["image", "image_digest", "build", "archive"] {
                let mut changed = made.clone();
                changed["spec"][field] = json!("changed");
                assert!(
                    !image_lacks(Some(&changed), "docker"),
                    "stale {field} observation"
                );
            }
            made["spec"]["image_facts"] = observed_image_facts(&made["spec"], &BTreeMap::new());
            assert!(
                !image_lacks(Some(&made), "docker"),
                "a failed check says nothing"
            );
        }
    }

    #[test]
    fn a_policy_forbids_a_runtime_or_requires_one() {
        assert!(check_policy(&json!({}), "kata").is_ok());
        assert!(check_policy(&json!({"gvisor": "forbid"}), "gvisor").is_err());
        assert!(check_policy(&json!({"gvisor": "forbid"}), "kata").is_ok());
        assert!(check_policy(&json!({"*": "forbid", "runc": "allow"}), "runc").is_ok());
        assert!(check_policy(&json!({"*": "forbid", "runc": "allow"}), "kata").is_err());
        let required = json!({"kata": "require"});
        assert!(check_policy(&required, "kata").is_ok());
        assert!(
            check_policy(&required, "runc")
                .unwrap_err()
                .contains("kata"),
            "the others may not be chosen, and the one required is named"
        );
        let two = json!({"kata": "require", "runsc": "require"});
        assert!(check_policy(&two, "runsc").is_ok() && check_policy(&two, "kata").is_ok());
    }

    #[test]
    fn a_hosts_default_and_aliases_cannot_escape_its_runtime_policy() {
        let descriptor = json!({"default_runtime": "runc", "runtimes": [
            {"name": "runc", "title": "runc", "aliases": ["container-shim"]},
            {"name": "kata", "title": "Kata", "boundary": "own_kernel"}
        ]});
        assert!(check_host_policy(&json!({"runc": "forbid"}), &descriptor, None).is_err());
        assert!(
            check_host_policy(
                &json!({"runc": "forbid"}),
                &descriptor,
                Some("container-shim")
            )
            .is_err()
        );
        assert!(
            check_host_policy(
                &json!({"runc": "allow", "container-shim": "forbid"}),
                &descriptor,
                Some("runc")
            )
            .is_err()
        );
        assert!(
            check_host_policy(
                &json!({"container-shim": "require"}),
                &descriptor,
                Some("runc")
            )
            .is_ok()
        );
        assert!(check_host_policy(&json!({"kata": "require"}), &descriptor, None).is_err());
        assert!(check_host_policy(&json!({"kata": "require"}), &descriptor, Some("kata")).is_ok());
        assert!(
            check_host_policy(
                &json!({"*": "forbid", "container-shim": "allow"}),
                &descriptor,
                None
            )
            .is_ok()
        );
        assert!(check_host_policy(&json!({"kata": "require"}), &json!({}), None).is_err());
        assert!(check_host_policy(&json!({}), &json!({}), None).is_ok());
    }

    #[test]
    fn a_template_limits_the_choice_but_always_permits_its_own_default() {
        let spec = json!({"runtime": "runc", "allowed_runtimes": ["kata"]});
        assert!(check_template(&spec, "kata").is_ok());
        assert!(check_template(&spec, "runc").is_ok());
        assert!(check_template(&spec, "runsc").is_err());
        assert!(check_template(&json!({"runtime": "runc"}), "runsc").is_ok());
        assert!(check_template(&json!({"allowed_runtimes": []}), "runsc").is_ok());
    }

    #[test]
    fn the_runtime_an_environment_chose_replaces_its_templates_default() {
        let mut spec = json!({"image": "x", "runtime": "runc"});
        apply_choice(&mut spec, Some("kata"));
        assert_eq!(spec["runtime"], "kata");
        apply_choice(&mut spec, None);
        assert_eq!(spec["runtime"], "kata");
    }

    #[test]
    fn the_unit_is_read_by_the_runtime_its_recorded_definition_names() {
        let drivers = json!({"docker": {
        "name": "docker", "network_egress": ["open"],
        "default_runtime": "runc",
        "runtimes": [
            {"name": "runc", "title": "runc", "boundary": "shared_kernel", "network_egress": ["open", "restricted"]},
            {"name": "kata", "title": "Kata", "boundary": "own_kernel", "network_egress": ["open"]}
        ]}});
        let made = json!({"spec": {"runtime": "kata"}});
        assert_eq!(definition_runtime(Some(&made)), Some("kata"));
        let effective = unit_driver(&drivers, "docker", Some(&made)).unwrap();
        assert_eq!(effective["network_egress"], json!(["open"]));
        let plain = unit_driver(&drivers, "docker", None).unwrap();
        assert_eq!(
            plain["network_egress"],
            json!(["open", "restricted"]),
            "the default class"
        );
        assert!(unit_driver(&drivers, "other", None).is_none());
    }

    #[test]
    fn an_agent_is_told_the_pair_the_boundary_and_only_the_limits_that_touch_it() {
        let class = gvisor();
        let plain = isolation_of(
            "docker",
            "Docker",
            Some(&class),
            Some("runsc"),
            Boundary::SharedKernel,
            &[],
        );
        assert_eq!(
            agent_line(&plain),
            "- Runs on Docker · gVisor (its own kernel written in user space). Here: to change a mount, the environment restarts (its units cannot be reached from the host)."
        );
        let with = isolation_of(
            "docker",
            "Docker",
            Some(&class),
            Some("runsc"),
            Boundary::SharedKernel,
            &["docker_inside"],
        );
        let line = agent_line(&with);
        assert!(
            line.contains("--network=host") && line.contains("-p and --expose do not work here"),
            "{line}"
        );
        assert!(!agent_line(&plain).contains("--network=host"));
        let mut kubernetes = plain.clone();
        kubernetes.limits = vec![
            RuntimeLimit {
                key: "kubernetes_mount_restart".into(),
                touches: vec![],
                todo: None,
                why: "To change a mount, the environment restarts.".into(),
            },
            RuntimeLimit {
                key: "kubernetes_device_restart".into(),
                touches: vec![],
                todo: None,
                why: "To change a device, the environment restarts.".into(),
            },
        ];
        assert!(agent_line(&kubernetes).contains("To change a mount, the environment restarts. To change a device, the environment restarts."));
        assert!(!agent_line(&kubernetes).contains(".."));
        kubernetes.limits.push(class.limits[0].clone());
        assert!(agent_line(&kubernetes).contains("To change a device, the environment restarts. To change a mount, the environment restarts (its units cannot be reached from the host)."));
        let none = isolation_of("host", "A directory", None, None, Boundary::Unbounded, &[]);
        assert_eq!(
            agent_line(&none),
            "- Runs on A directory (the machine itself)."
        );
    }

    #[test]
    fn what_a_runtime_cannot_give_is_said_only_for_what_the_environment_could_ask() {
        let class = gvisor();
        let kvm = isolation_of(
            "docker",
            "Docker",
            Some(&class),
            None,
            Boundary::SharedKernel,
            &["kvm"],
        );
        assert_eq!(kvm.unavailable.len(), 1);
        let gpu = isolation_of(
            "docker",
            "Docker",
            Some(&class),
            None,
            Boundary::SharedKernel,
            &["gpu"],
        );
        assert!(gpu.unavailable.is_empty());
    }

    #[test]
    fn an_agent_is_told_after_a_replacement_whether_what_it_installed_came_across() {
        assert!(
            replaced_line("a capability was granted", false).contains("is gone; install it again")
        );
        assert!(replaced_line("an update", true).contains("is kept"));
    }

    #[test]
    fn a_unit_of_a_virtual_machine_has_longer_to_enroll_than_the_installations_figure() {
        let drivers = json!({"docker": {"runtimes": [
            {"name": "runc", "title": "runc"},
            {"name": "kata", "title": "Kata", "boundary": "own_kernel", "enrol_timeout_secs": 300}
        ]}});
        let ordinary = std::time::Duration::from_secs(120);
        let secs = |runtime: &str| {
            enroll_time(ordinary, &drivers, "docker", &json!({"runtime": runtime})).as_secs()
        };
        assert_eq!(secs("kata"), 300);
        assert_eq!(secs("runc"), 120);
        assert_eq!(secs("unknown"), 120);
        let long = std::time::Duration::from_secs(600);
        assert_eq!(
            enroll_time(long, &drivers, "docker", &json!({"runtime": "kata"})).as_secs(),
            600
        );
    }

    #[test]
    fn a_capability_card_states_the_risk_under_the_runtime_and_the_limit_that_touches_it() {
        let mut class = gvisor();
        class.risks.insert("docker_inside".into(), "medium".into());
        let descriptor = json!({"runtimes": [serde_json::to_value(&class).unwrap()], "default_runtime": "runsc"});
        let (risk, limits) = capability_words(&descriptor, "docker_inside", "high");
        assert_eq!(risk, "medium");
        assert_eq!(
            limits,
            [
                "Run containers you start inside with --network=host to serve a port (-p and --expose do not work here)"
            ]
        );
        let (risk, limits) = capability_words(&descriptor, "ptrace", "medium");
        assert_eq!((risk.as_str(), limits.len()), ("medium", 0));
        assert_eq!(
            capability_words(&json!({}), "kvm", "high"),
            ("high".to_owned(), Vec::new())
        );
    }

    #[test]
    fn mounts_are_a_subject_only_where_there_are_some() {
        assert_eq!(subjects(["ptrace", "ptrace"], true), ["mounts", "ptrace"]);
        assert_eq!(subjects(["ptrace"], false), ["ptrace"]);
    }
}
