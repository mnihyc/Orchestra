//! Images on a real Docker daemon: pulled by reference and pinned by digest, built from a Dockerfile once for each
//! definition, loaded from an archive once, and refused with a typed error when they are not to be had.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::io::Write;
use std::time::Duration;

use common::{ALPINE, BUSYBOX, Lab};
use futures::TryStreamExt;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::sources::{BuildSource, Source};
use orch_driver_docker::build::{definition_alias, spec_alias};
use orch_driver_docker::progress::{Collected, Phase};
use orch_types::Json;
use serde_json::json;

/// Builds leave unnamed layers while they run, so the tests that look for what a build left behind take turns with
/// the tests that build.
static BUILDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The images without a name that were made since `since` by a build of this lab, which put its token in the
/// environment of everything it made (see [`marked`]): what a build that did not finish would leave. Images that
/// other people's builds make on this daemon meanwhile carry no such token and are not looked at.
async fn steps_since(lab: &Lab, since: chrono::DateTime<chrono::Utc>) -> Vec<String> {
    let options = bollard::query_parameters::ListImagesOptionsBuilder::default()
        .all(true)
        .build();
    let images = lab.docker.list_images(Some(options)).await.unwrap();
    let mark = format!("ORCH_TEST_MARK={}", lab.token);
    let mut mine = Vec::new();
    for image in images.into_iter().filter(|image| {
        image.repo_tags.iter().all(|tag| tag == "<none>:<none>")
            && image.repo_digests.is_empty()
            && image.created >= since.timestamp() - 1
    }) {
        let Ok(inspected) = lab.docker.inspect_image(&image.id).await else {
            continue;
        };
        let env = inspected
            .config
            .and_then(|config| config.env)
            .unwrap_or_default();
        if env.contains(&mark) {
            mine.push(image.id);
        }
    }
    mine
}

fn dockerfile(lines: &str) -> String {
    format!("FROM busybox:latest\n{lines}\n")
}

/// A Dockerfile whose first step puts the lab's token in the environment, so that the images its build makes are
/// told from anybody else's.
fn marked(lab: &Lab, lines: &str) -> String {
    format!(
        "FROM busybox:latest\nENV ORCH_TEST_MARK={}\n{lines}\n",
        lab.token
    )
}

/// Registers the names a build of this spec has, so that the lab removes them.
fn track_build(lab: &Lab, spec: &Json, identity: Option<&str>) -> (String, String) {
    let Source::Build(build) = Source::from_spec(spec).unwrap() else {
        panic!("a build")
    };
    let definition = definition_alias(&lab.config, &build.definition_hash(identity));
    let alias = spec_alias(&lab.config, &build);
    lab.track_image(&definition);
    lab.track_image(&alias);
    (definition, alias)
}

#[tokio::test]
async fn pulling_an_image_that_is_not_to_be_had_fails_with_an_image_error() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let error = lab
        .driver
        .ensure_image(&json!({ "image": "localhost:1/orch-test/missing:never" }))
        .await
        .unwrap_err();
    assert!(
        matches!(error, DriverError::Image(ref why) if why.contains("localhost:1/orch-test/missing:never")),
        "{error:?}"
    );
    assert_eq!(error.code(), "image_failed");
    assert!(!error.retryable());
    let none_named = lab
        .driver
        .ensure_image(&json!({ "runtime": "runc" }))
        .await
        .unwrap_err();
    assert!(
        matches!(none_named, DriverError::InvalidSpec(_)),
        "{none_named:?}"
    );
}

#[tokio::test]
async fn a_pinned_reference_is_the_image_at_that_digest_and_an_unpinned_one_is_looked_up_again() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let held = lab.docker.inspect_image(BUSYBOX).await.unwrap();
    let digest = held.repo_digests.unwrap()[0]
        .split_once('@')
        .unwrap()
        .1
        .to_owned();
    let pinned = lab
        .driver
        .ensure_image(&json!({ "image": BUSYBOX, "image_digest": digest }))
        .await
        .unwrap();
    assert_eq!(pinned.digest.as_deref(), Some(digest.as_str()));
    // A unit made from the pinned spec runs exactly that image.
    let spec = lab.request("environment", "pinned", |request| {
        request["spec"]["image_digest"] = json!(digest)
    });
    let unit = lab.run(&spec).await;
    let image = lab.inspect(&unit).await.config.unwrap().image.unwrap();
    assert_eq!(image, format!("busybox@{digest}"));
    let unpinned = lab
        .driver
        .ensure_image(&json!({ "image": BUSYBOX }))
        .await
        .unwrap();
    assert!(
        unpinned
            .digest
            .is_some_and(|found| found.starts_with("sha256:"))
    );
}

#[tokio::test]
async fn a_registry_that_cannot_be_reached_leaves_the_image_the_host_holds_in_use() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let reference = format!("localhost:1/orch-test-{}/held:1", lab.token);
    lab.track_image(&reference);
    lab.docker
        .tag_image(
            BUSYBOX,
            Some(
                bollard::query_parameters::TagImageOptionsBuilder::default()
                    .repo(&format!("localhost:1/orch-test-{}/held", lab.token))
                    .tag("1")
                    .build(),
            ),
        )
        .await
        .unwrap();
    let ensured = lab
        .driver
        .ensure_image(&json!({ "image": reference }))
        .await
        .unwrap();
    assert!(ensured.digest.is_some());
    // The same image pinned is not looked up at all.
    let digest = ensured.digest.unwrap();
    let pinned = lab
        .driver
        .ensure_image(&json!({ "image": reference, "image_digest": digest }))
        .await
        .unwrap();
    assert_eq!(pinned.digest.as_deref(), Some(digest.as_str()));
}

#[tokio::test]
async fn available_image_programs_are_observed_without_running_its_entrypoint_or_programs() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = json!({
        "build": { "dockerfile": dockerfile(concat!(
            "RUN printf '#!/bin/sh\\nexit 99\\n' > /bin/docker && ",
            "cp /bin/docker /bin/dockerd && chmod 755 /bin/docker /bin/dockerd\n",
            "ENTRYPOINT [\"/bin/sh\",\"-c\",\"exit 77\"]",
        )) },
        "image_facts": { "docker": false, "qemu": true },
    });
    track_build(&lab, &spec, None);
    let made = lab.driver.ensure_image(&spec).await.unwrap();
    assert_eq!(made.programs.get("docker"), Some(&true));
    assert_eq!(made.programs.get("qemu"), Some(&false));
    let cached = lab.driver.ensure_image(&spec).await.unwrap();
    assert_eq!(
        cached.programs, made.programs,
        "cached images are inspected too"
    );
    let filters = std::collections::HashMap::from([(
        "label".to_owned(),
        vec![format!("orch.test={}", lab.token)],
    )]);
    let left = lab
        .docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await
        .unwrap();
    assert!(
        left.is_empty(),
        "the inspection leaves no containers: {left:?}"
    );
}

#[tokio::test]
async fn a_dockerfile_is_built_once_for_each_definition_and_found_again() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = json!({ "build": { "dockerfile": dockerfile("ARG WHO=nobody\nRUN echo built-for-$WHO > /built.txt"), "context": null, "args": { "WHO": "tests" }, "target": null } });
    let (definition, alias) = track_build(&lab, &spec, None);
    let progress = Collected::default();
    let built = lab
        .driver
        .ensure_image_reporting(&spec, &progress)
        .await
        .unwrap();
    assert!(
        built
            .digest
            .as_deref()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    let record = built.built.clone().unwrap();
    assert_eq!(record.commit, None);
    assert_eq!(record.bases.len(), 1);
    assert_eq!(record.bases[0].reference, "busybox:latest");
    assert!(record.bases[0].digest.starts_with("sha256:"));
    let lines = progress.lines();
    assert!(lines.iter().all(|(phase, _)| *phase == Phase::Build));
    assert!(
        lines
            .iter()
            .any(|(_, line)| line.contains("Step 3/3") || line.contains("RUN echo")),
        "{lines:?}"
    );
    assert!(
        lab.docker.inspect_image(&definition).await.is_ok()
            && lab.docker.inspect_image(&alias).await.is_ok()
    );
    let labels = lab
        .docker
        .inspect_image(&definition)
        .await
        .unwrap()
        .config
        .unwrap()
        .labels
        .unwrap();
    assert_eq!(labels["orch.driver"], "docker");
    assert_eq!(labels["orch.test"], lab.token);
    // The same definition is found again and built no more.
    let quiet = Collected::default();
    let again = lab
        .driver
        .ensure_image_reporting(&spec, &quiet)
        .await
        .unwrap();
    assert_eq!(again, built);
    assert!(quiet.lines().is_empty(), "{:?}", quiet.lines());
    // A unit made from the spec runs the built image.
    let request = lab.request("environment", "built", |request| {
        request["spec"] =
            json!({ "build": spec["build"].clone(), "user": { "uid": 1000, "gid": 1000 } });
        request["env"] = json!({ "LAB_RUN": "cat /built.txt > $ORCH_DATA_DIR/seen.txt" });
    });
    let unit = lab.run(&request).await;
    assert_eq!(
        lab.read(&unit, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("built-for-tests\n")
    );
    // Another definition is another build, and the spec's own name follows it.
    let changed = json!({ "build": { "dockerfile": dockerfile("ARG WHO=nobody\nRUN echo built-for-$WHO > /built.txt"), "context": null, "args": { "WHO": "everyone" }, "target": null } });
    let (other_definition, other_alias) = track_build(&lab, &changed, None);
    assert_ne!(other_definition, definition);
    let rebuilt = lab
        .driver
        .ensure_image_reporting(&changed, &Collected::default())
        .await
        .unwrap();
    assert_ne!(rebuilt.digest, built.digest);
    assert_ne!(other_alias, alias);
    // A build made again from the same bases finds its steps in the cache of the builder: it is the same image, and
    // nothing is kept beside it.
    let forced = Collected::default();
    let again = lab.driver.rebuild_image(&spec, &forced).await.unwrap();
    assert!(!forced.lines().is_empty());
    assert_eq!(again.digest, built.digest);
    assert!(
        named_like(&lab, &format!("{}:old-", lab.config.image_repository))
            .await
            .is_empty()
    );
}

/// The names the host has for images that begin with `prefix`.
async fn named_like(lab: &Lab, prefix: &str) -> Vec<String> {
    let listed = lab
        .docker
        .list_images(Some(
            bollard::query_parameters::ListImagesOptionsBuilder::default().build(),
        ))
        .await
        .unwrap();
    listed
        .into_iter()
        .flat_map(|image| image.repo_tags)
        .filter(|tag| tag.starts_with(prefix))
        .collect()
}

/// Gives an image of the host one more name.
async fn name_image(lab: &Lab, image: &str, name: &str) {
    let (repository, tag) = name.rsplit_once(':').unwrap();
    lab.docker
        .tag_image(
            image,
            Some(
                bollard::query_parameters::TagImageOptionsBuilder::default()
                    .repo(repository)
                    .tag(tag)
                    .build(),
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_superseded_build_and_its_layers_stay_until_the_exact_build_is_selected_for_removal() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(ALPINE).await;
    // A base of the lab's own, which the registry named in it cannot be asked about, so that what the host holds is
    // what the build uses; it is first busybox and then alpine.
    let base = format!("localhost:1/orch-test-{}/base:1", lab.token);
    lab.track_image(&base);
    name_image(&lab, BUSYBOX, &base).await;
    let spec = json!({ "build": { "dockerfile": format!("FROM {base}\nENV ORCH_TEST_MARK={}\nRUN echo first > /which.txt", lab.token), "context": null, "args": {}, "target": null } });
    let (_, alias) = track_build(&lab, &spec, None);
    let started = chrono::Utc::now();
    let first = lab.driver.ensure_image(&spec).await.unwrap();
    // A unit stands on the first build.
    let request = lab.request("environment", "on-old", |request| {
        request["spec"] =
            json!({ "build": spec["build"].clone(), "user": { "uid": 1000, "gid": 1000 } });
    });
    let unit = lab.create(&request).await;
    // The base changes under the same name, and the build is made again.
    name_image(&lab, ALPINE, &base).await;
    let second = lab
        .driver
        .rebuild_image(&spec, &Collected::default())
        .await
        .unwrap();
    assert_ne!(second.digest, first.digest);
    // What the build records of its base is the base it was made over this time.
    assert_ne!(
        second.built.unwrap().bases[0].digest,
        first.built.unwrap().bases[0].digest
    );
    // The name follows the new build, the unit keeps the old one, and the old one is found by a name of its own.
    let prefix = format!("{}:old-", lab.config.image_repository);
    let old = named_like(&lab, &prefix).await;
    assert_eq!(old.len(), 1, "{old:?}");
    lab.track_image(&old[0]);
    let held = lab.inspect(&unit).await;
    assert_eq!(
        held.image
            .as_deref()
            .map(|id| id.trim_start_matches("sha256:")),
        lab.docker
            .inspect_image(&old[0])
            .await
            .unwrap()
            .id
            .as_deref()
            .map(|id| id.trim_start_matches("sha256:"))
    );
    assert!(lab.driver.remove_image(&old[0]).await.is_err());
    assert!(lab.docker.inspect_image(&old[0]).await.is_ok());
    // A completed build's layers are cache objects; selecting its image does not select its parents.
    let current = lab.docker.inspect_image(&alias).await.unwrap().id.unwrap();
    let kept = chain_of(&lab, &current).await;
    let parents = chain_of(&lab, &old[0]).await;
    let mut before: Vec<_> = steps_since(&lab, started)
        .await
        .into_iter()
        .filter(|id| !kept.contains(id))
        .collect();
    before.sort_unstable();
    lab.driver.remove(&unit).await.unwrap();
    assert_eq!(
        lab.driver.remove_image(&old[0]).await.unwrap(),
        orch_driver_docker::images::Removal::Gone
    );
    assert!(lab.docker.inspect_image(&old[0]).await.is_err());
    for parent in parents.iter().skip(1) {
        assert!(
            lab.docker.inspect_image(parent).await.is_ok(),
            "unselected parent {parent} stays"
        );
    }
    assert_eq!(
        lab.docker
            .inspect_image(&alias)
            .await
            .unwrap()
            .id
            .as_deref(),
        Some(current.as_str())
    );
    let mut after: Vec<_> = steps_since(&lab, started)
        .await
        .into_iter()
        .filter(|id| !kept.contains(id))
        .collect();
    after.sort_unstable();
    assert_eq!(
        after, before,
        "unselected cache layers are retained, not silently pruned"
    );
}

/// The image with this id and the ones beneath it that nothing has named: the steps of its build.
async fn chain_of(lab: &Lab, id: &str) -> Vec<String> {
    let options = bollard::query_parameters::ListImagesOptionsBuilder::default()
        .all(true)
        .build();
    let images = lab.docker.list_images(Some(options)).await.unwrap();
    let mut chain = vec![id.to_owned()];
    let mut current = id.to_owned();
    while let Some(image) = images.iter().find(|image| image.id == current) {
        if image.parent_id.is_empty() {
            break;
        }
        chain.push(image.parent_id.clone());
        current = image.parent_id.clone();
    }
    chain
}

#[tokio::test]
async fn builds_stay_until_the_server_selects_each_exact_inventory_object() {
    use orch_driver_contract::resources::ResourceKind;
    let Some(lab) = Lab::with(|config, _| {
        let token = config.labels["orch.test"].clone();
        config
            .labels
            .insert("orch.system".into(), format!("orch-test-{token}"));
    })
    .await
    else {
        return;
    };
    let _building = BUILDING.lock().await;
    let older = json!({ "build": { "dockerfile": dockerfile("RUN echo older > /which.txt"), "context": null, "args": {}, "target": null } });
    let newer = json!({ "build": { "dockerfile": dockerfile("RUN echo newer > /which.txt"), "context": null, "args": {}, "target": null } });
    let (older_definition, older_alias) = track_build(&lab, &older, None);
    let (newer_definition, newer_alias) = track_build(&lab, &newer, None);
    lab.driver.ensure_image(&older).await.unwrap();
    lab.driver.ensure_image(&newer).await.unwrap();
    for name in [
        &older_definition,
        &older_alias,
        &newer_definition,
        &newer_alias,
    ] {
        assert!(
            lab.docker.inspect_image(name).await.is_ok(),
            "publishing another build never chooses a cache fate"
        );
    }
    let older_id = lab
        .docker
        .inspect_image(&older_definition)
        .await
        .unwrap()
        .id
        .unwrap();
    let newer_id = lab
        .docker
        .inspect_image(&newer_definition)
        .await
        .unwrap()
        .id
        .unwrap();
    let resources = lab.driver.resources(&lab.system).await.unwrap();
    let selected = resources
        .iter()
        .find(|item| item.resource.kind == ResourceKind::Build && item.resource.id == older_id)
        .unwrap();
    lab.driver
        .remove_resource(&lab.system, &selected.resource)
        .await
        .unwrap();
    assert!(lab.docker.inspect_image(&older_definition).await.is_err());
    assert!(lab.docker.inspect_image(&older_alias).await.is_err());
    for name in [&newer_definition, &newer_alias] {
        assert_eq!(
            lab.docker.inspect_image(name).await.unwrap().id.as_deref(),
            Some(newer_id.as_str()),
            "the unselected newer build is unchanged"
        );
    }
    assert!(lab.docker.inspect_image(BUSYBOX).await.is_ok());
    let remaining = lab.driver.resources(&lab.system).await.unwrap();
    assert!(!remaining.iter().any(|item| item.resource.id == older_id));
    let selected = remaining
        .iter()
        .find(|item| item.resource.kind == ResourceKind::Build && item.resource.id == newer_id)
        .unwrap();
    lab.driver
        .remove_resource(&lab.system, &selected.resource)
        .await
        .unwrap();
    assert!(lab.docker.inspect_image(&newer_definition).await.is_err());
    assert!(lab.docker.inspect_image(&newer_alias).await.is_err());
    assert!(
        lab.docker.inspect_image(BUSYBOX).await.is_ok(),
        "no host-wide or parent prune accompanies exact removal"
    );
}

#[tokio::test]
async fn a_failed_build_keeps_the_end_of_its_log_in_its_error() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = json!({ "build": { "dockerfile": marked(&lab, "RUN echo first > /first.txt\nRUN echo marker-line-one; echo marker-line-two; exit 5"), "context": null, "args": {}, "target": null } });
    let (definition, _) = track_build(&lab, &spec, None);
    let began = chrono::Utc::now();
    let error = lab.driver.ensure_image(&spec).await.unwrap_err();
    let DriverError::Image(why) = error else {
        panic!("an image error: {error:?}")
    };
    assert!(why.contains("marker-line-two"), "{why}");
    assert!(why.contains("non-zero code: 5"), "{why}");
    assert!(
        lab.docker.inspect_image(&definition).await.is_err(),
        "nothing is kept of a build that failed"
    );
    assert_eq!(
        steps_since(&lab, began).await,
        Vec::<String>::new(),
        "the steps of a build that failed are removed with it"
    );
}

#[tokio::test]
async fn a_build_that_takes_longer_than_the_host_allows_is_stopped() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    let Some(slow) = Lab::with(|config, _| config.build_timeout_secs = 6).await else {
        return;
    };
    drop(lab);
    let spec = json!({ "build": { "dockerfile": marked(&slow, "RUN echo first > /first.txt\nRUN sleep 3571"), "context": null, "args": {}, "target": null } });
    track_build(&slow, &spec, None);
    let began = chrono::Utc::now();
    let started = std::time::Instant::now();
    let error = slow.driver.ensure_image(&spec).await.unwrap_err();
    assert!(
        matches!(error, DriverError::Image(ref why) if why.contains("took longer than the 6 seconds")),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(40),
        "it was stopped at the limit"
    );
    // The builder does not go on with what it was told to stop.
    let mut leftover = true;
    for _ in 0..40 {
        let running = slow
            .docker
            .list_containers(None::<bollard::query_parameters::ListContainersOptions>)
            .await
            .unwrap();
        leftover = running.iter().any(|container| {
            container
                .command
                .as_deref()
                .is_some_and(|command| command.contains("sleep 3571"))
        });
        if !leftover {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        !leftover,
        "the step of the build that was stopped is still running"
    );
    assert_eq!(
        steps_since(&slow, began).await,
        Vec::<String>::new(),
        "the layers of a build that was stopped are removed with it"
    );
}

fn tar_of_files(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, path, content.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap()
}

#[tokio::test]
async fn a_build_takes_the_files_of_its_context_from_an_uploaded_tar_a_tar_gz_or_a_zip() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    let plain = tar_of_files(&[("hello.txt", "hello from a tar\n")]);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar_of_files(&[("hello.txt", "hello from a tar.gz\n")]))
        .unwrap();
    let gzipped = gz.finish().unwrap();
    let mut zipped = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zipped));
        writer
            .start_file("hello.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"hello from a zip\n").unwrap();
        writer.finish().unwrap();
    }
    for (label, bytes, expected) in [
        ("tar", plain, "hello from a tar\n"),
        ("targz", gzipped, "hello from a tar.gz\n"),
        ("zip", zipped, "hello from a zip\n"),
    ] {
        let blob = lab.blobs.put(bytes);
        let spec = json!({ "build": { "dockerfile": dockerfile("COPY hello.txt /hello.txt"), "context": { "blob": blob.to_string() }, "args": {}, "target": null } });
        track_build(&lab, &spec, Some(&blob.to_string()));
        lab.driver.ensure_image(&spec).await.unwrap();
        let request = lab.request("environment", label, |request| {
            request["spec"] =
                json!({ "build": spec["build"].clone(), "user": { "uid": 1000, "gid": 1000 } });
            request["env"] = json!({ "LAB_RUN": "cat /hello.txt > $ORCH_DATA_DIR/seen.txt" });
        });
        let unit = lab.run(&request).await;
        assert_eq!(
            lab.read(&unit, "/var/lib/orch/seen.txt").await.as_deref(),
            Some(expected),
            "{label}"
        );
        lab.driver
            .stop(&unit, Duration::from_secs(3))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_build_takes_the_files_of_its_context_from_a_git_repository_at_the_commit_its_reference_names()
 {
    let _building = BUILDING.lock().await;
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: git is not installed");
        return;
    }
    let Some(lab) = Lab::with(|config, _| config.git_schemes.push("file".to_owned())).await else {
        return;
    };
    let repository = lab.path("repository");
    let git = |arguments: &[&str]| {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(arguments)
            .current_dir(&repository)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init", "--quiet"]);
    std::fs::create_dir_all(repository.join("app")).unwrap();
    std::fs::write(repository.join("app/data.txt"), "first\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "first"]);
    let first = git(&["rev-parse", "HEAD"]);
    let url = format!("file://{}", repository.display());
    let spec = json!({ "build": { "dockerfile": dockerfile("COPY data.txt /data.txt"),
        "context": { "git": url, "ref": "main", "path": "app" }, "args": {}, "target": null } });
    track_build(&lab, &spec, Some(&first));
    let built = lab.driver.ensure_image(&spec).await.unwrap();
    assert_eq!(built.built.unwrap().commit.as_deref(), Some(first.as_str()));
    // A branch is resolved at each build: a new commit is a new definition.
    std::fs::write(repository.join("app/data.txt"), "second\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "second"]);
    let second = git(&["rev-parse", "HEAD"]);
    track_build(&lab, &spec, Some(&second));
    let again = lab.driver.ensure_image(&spec).await.unwrap();
    assert_eq!(
        again.built.unwrap().commit.as_deref(),
        Some(second.as_str())
    );
    let request = lab.request("environment", "from-git", |request| {
        request["spec"] =
            json!({ "build": spec["build"].clone(), "user": { "uid": 1000, "gid": 1000 } });
        request["env"] = json!({ "LAB_RUN": "cat /data.txt > $ORCH_DATA_DIR/seen.txt" });
    });
    let unit = lab.run(&request).await;
    assert_eq!(
        lab.read(&unit, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("second\n")
    );
    // An address that needs a credential, or reaches into this machine, is refused.
    let refused = Lab::start().await.unwrap();
    let bad = json!({ "build": { "dockerfile": dockerfile("RUN true"), "context": { "git": "file:///etc", "ref": "main", "path": "." }, "args": {}, "target": null } });
    assert!(matches!(
        refused.driver.ensure_image(&bad).await,
        Err(DriverError::InvalidSpec(_))
    ));
}

#[tokio::test]
async fn an_archive_is_loaded_once_for_each_blob_and_a_unit_runs_the_image_in_it() {
    let _building = BUILDING.lock().await;
    let Some(lab) = Lab::start().await else {
        return;
    };
    // An image to export: built here, then taken away, so that the archive is the only place it comes from.
    let source = json!({ "build": { "dockerfile": dockerfile("RUN echo from-the-archive > /archived.txt"), "context": null, "args": {}, "target": null } });
    let (definition, alias) = track_build(&lab, &source, None);
    lab.driver.ensure_image(&source).await.unwrap();
    let pieces: Vec<bytes::Bytes> = lab
        .docker
        .export_image(&definition)
        .try_collect()
        .await
        .unwrap();
    let archive: Vec<u8> = pieces
        .iter()
        .flat_map(|piece| piece.iter().copied())
        .collect();
    for tag in [&alias, &definition] {
        lab.driver.remove_image(tag).await.unwrap();
    }
    assert!(lab.docker.inspect_image(&definition).await.is_err());
    let blob = lab.blobs.put(archive);
    let spec = json!({ "archive": { "blob": blob.to_string(), "image": null } });
    let alias = orch_driver_docker::images::archive_alias(
        &lab.config,
        &orch_driver_contract::sources::ArchiveSource { blob, image: None },
    );
    lab.track_image(&alias);
    let loaded = lab
        .driver
        .ensure_image_reporting(&spec, &Collected::default())
        .await
        .unwrap();
    assert!(loaded.digest.is_some());
    assert_eq!(lab.blobs.opened(&blob), 1);
    let again = lab.driver.ensure_image(&spec).await.unwrap();
    assert_eq!(again, loaded);
    assert_eq!(lab.blobs.opened(&blob), 1, "an archive is loaded once");
    let request = lab.request("environment", "from-archive", |request| {
        request["spec"] =
            json!({ "archive": spec["archive"].clone(), "user": { "uid": 1000, "gid": 1000 } });
        request["env"] = json!({ "LAB_RUN": "cat /archived.txt > $ORCH_DATA_DIR/seen.txt" });
    });
    let unit = lab.run(&request).await;
    assert_eq!(
        lab.read(&unit, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("from-the-archive\n")
    );
    // A blob that is not an image archive is an image error.
    let junk = lab.blobs.put(vec![9_u8; 3000]);
    let error = lab
        .driver
        .ensure_image(&json!({ "archive": { "blob": junk.to_string(), "image": null } }))
        .await
        .unwrap_err();
    assert!(matches!(error, DriverError::Image(_)), "{error:?}");
    let _ = BuildSource::clone;
}

#[tokio::test]
async fn a_host_that_does_not_build_refuses_a_build_and_does_not_offer_the_source() {
    let Some(lab) = Lab::with(|config, _| config.builds = false).await else {
        return;
    };
    let spec = json!({ "build": { "dockerfile": dockerfile("RUN true"), "context": null, "args": {}, "target": null } });
    assert!(matches!(
        lab.driver.ensure_image(&spec).await,
        Err(DriverError::Unsupported(_))
    ));
    assert!(matches!(
        lab.driver
            .validate(
                orch_driver_contract::runtime::RuntimeKind::Environment,
                &spec
            )
            .await,
        Err(DriverError::InvalidSpec(_))
    ));
    assert!(
        !lab.driver
            .describe()
            .has_source(orch_driver_contract::sources::SourceKind::Build)
    );
}
