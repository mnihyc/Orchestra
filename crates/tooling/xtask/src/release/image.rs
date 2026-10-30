//! The default environment image and the developer image made from it: each built from its recipe and held to what the
//! recipe pins.
//!
//! The recipe pins the version of everything it installs beside the distribution (Node.js, the
//! browser's command line and the browser that command line asks for, the VNC server, the window
//! manager) and writes each into a label of the image. A build here reads the labels back, refuses an
//! image that does not say what the recipe pins, and prints what it holds, so that the notes of a
//! release can say which versions it ships.

use crate::release::manifest::Image;
use crate::release::{ReleaseError, capture, run};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

/// What the image is for, and the name of its tag.
pub const NAME: &str = "orch-env-base";
/// The recipe, below the root of the repository.
const RECIPE: &str = "images/orch-env-base";
/// What the developer image is for, and the name of its tag: the default image with Docker, QEMU and build tools.
pub const DEVELOPER_NAME: &str = "orch-env-dev";
/// Its recipe, below the root of the repository.
const DEVELOPER_RECIPE: &str = "images/orch-env-dev";
/// The recipe's build file, inside it.
const BUILD_FILE: &str = "Dockerfile";
/// The longest a Docker tag may be.
const LONGEST_TAG: usize = 128;

/// A version the recipe pins: the build argument that holds it and the label that says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    /// The build argument of the recipe, which gives the version as its default.
    pub argument: &'static str,
    /// The label of the image that carries the version.
    pub label: &'static str,
}

/// Everything the recipe pins: what the image holds beside what the distribution gives it.
pub const PINS: [Pin; 7] = [
    Pin {
        argument: "NODE_VERSION",
        label: "orch.image.node",
    },
    Pin {
        argument: "PLAYWRIGHT_CLI_VERSION",
        label: "orch.image.playwright-cli",
    },
    Pin {
        argument: "PLAYWRIGHT_VERSION",
        label: "orch.image.playwright",
    },
    Pin {
        argument: "BROWSER_REVISION",
        label: "orch.image.browser.revision",
    },
    Pin {
        argument: "BROWSER_VERSION",
        label: "orch.image.browser.version",
    },
    Pin {
        argument: "TIGERVNC_VERSION",
        label: "orch.image.tigervnc",
    },
    Pin {
        argument: "OPENBOX_VERSION",
        label: "orch.image.openbox",
    },
];

/// Everything the developer recipe pins: what it adds to the default image.
pub const DEVELOPER_PINS: [Pin; 5] = [
    Pin {
        argument: "DOCKER_VERSION",
        label: "orch.image.docker",
    },
    Pin {
        argument: "CONTAINERD_VERSION",
        label: "orch.image.containerd",
    },
    Pin {
        argument: "BUILDX_VERSION",
        label: "orch.image.buildx",
    },
    Pin {
        argument: "COMPOSE_VERSION",
        label: "orch.image.compose",
    },
    Pin {
        argument: "QEMU_VERSION",
        label: "orch.image.qemu",
    },
];

/// What `cargo xtask image` is asked.
#[derive(clap::Args, Debug)]
pub struct ImageArgs {
    /// The version the image is tagged with: `orch-env-base:<VERSION>`. A release tags
    /// its image with its own version; a try of the recipe may tag it with any name.
    #[arg(long, default_value = "dev")]
    version: String,
    /// Build without Docker's layer cache, so that the image is what the recipe says today
    /// (a download that moved, a package that Debian replaced) and not what an earlier build left.
    #[arg(long)]
    fresh: bool,
}

/// Builds the image alone and says what it holds.
pub fn command(root: &Path, args: &ImageArgs) -> Result<(), ReleaseError> {
    let image = build(root, &args.version, args.fresh)?;
    eprintln!("{} is {}", image.reference, image.digest);
    Ok(())
}

/// The versions of `pins` that a build file gives as the defaults of its build arguments: each pin with the version
/// that is the default of its build argument.
pub fn pinned_of(pins: &[Pin], build_file: &str) -> Result<Vec<(Pin, String)>, ReleaseError> {
    pins.iter()
        .map(|pin| {
            let prefix = format!("ARG {}=", pin.argument);
            build_file
                .lines()
                .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .map(|version| (*pin, version.to_owned()))
                .ok_or_else(|| {
                    ReleaseError::Invalid(format!(
                        "the recipe does not pin {} (a line `ARG {}=<version>` in {BUILD_FILE})",
                        pin.label, pin.argument
                    ))
                })
        })
        .collect()
}

/// What is wrong with the labels of an image, set against what the recipe pins: a label that is
/// missing, or that says another version.
#[must_use]
pub fn problems(pinned: &[(Pin, String)], labels: &BTreeMap<String, String>) -> Vec<String> {
    pinned
        .iter()
        .filter_map(|(pin, version)| match labels.get(pin.label) {
            Some(said) if said == version => None,
            Some(said) => Some(format!(
                "the label {} says {said}, and the recipe pins {version}",
                pin.label
            )),
            None => Some(format!(
                "the image has no label {} (the recipe pins {version})",
                pin.label
            )),
        })
        .collect()
}

/// Whether `version` can be the second half of a Docker tag.
fn is_tag(version: &str) -> bool {
    let mut characters = version.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric() || first == '_')
        && version.len() <= LONGEST_TAG
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The labels of the image `reference`, as the daemon holds them.
fn labels_of(reference: &str) -> Result<BTreeMap<String, String>, ReleaseError> {
    let json = capture(Command::new("docker").args([
        "image",
        "inspect",
        "--format",
        "{{json .Config.Labels}}",
        reference,
    ]))?;
    let labels: Option<BTreeMap<String, String>> =
        serde_json::from_str(&json).map_err(|error| {
            ReleaseError::Invalid(format!("the labels of {reference} are not JSON: {error}"))
        })?;
    Ok(labels.unwrap_or_default())
}

/// Builds the image from its recipe as `orch-env-base:<version>`, checks that its labels say what the
/// recipe pins, prints what it holds, and names it by its content.
pub fn build(root: &Path, version: &str, fresh: bool) -> Result<Image, ReleaseError> {
    build_recipe(root, NAME, RECIPE, &PINS, &[], version, fresh)
}

/// Builds the developer image from its recipe as `orch-env-dev:<version>`, on the default image of the same version
/// (which must have been built before), and holds it to what its recipe pins as the default image is held.
pub fn build_developer(root: &Path, version: &str, fresh: bool) -> Result<Image, ReleaseError> {
    build_recipe(
        root,
        DEVELOPER_NAME,
        DEVELOPER_RECIPE,
        &DEVELOPER_PINS,
        &[format!("BASE_IMAGE={NAME}:{version}")],
        version,
        fresh,
    )
}

/// Builds the image `name:<version>` from the recipe below `recipe_dir` with the build arguments `arguments` besides the
/// version, checks that its labels say what `pins` ask, prints what it holds, and names it by its content.
fn build_recipe(
    root: &Path,
    name: &str,
    recipe_dir: &str,
    pins: &[Pin],
    arguments: &[String],
    version: &str,
    fresh: bool,
) -> Result<Image, ReleaseError> {
    if !is_tag(version) {
        return Err(ReleaseError::Invalid(format!(
            "{version:?} cannot be a tag of the image (letters, digits, `_`, `.` and `-`)"
        )));
    }
    let recipe = root.join(recipe_dir);
    let reference = format!("{name}:{version}");
    let mut docker = Command::new("docker");
    docker
        .args(["build", "--tag", &reference])
        .args(["--build-arg", &format!("VERSION={version}")]);
    for argument in arguments {
        docker.args(["--build-arg", argument]);
    }
    if fresh {
        docker.arg("--no-cache");
    }
    run(docker.arg(&recipe))?;

    let build_file = recipe.join(BUILD_FILE);
    let text = fs::read_to_string(&build_file).map_err(|source| ReleaseError::Io {
        path: build_file,
        source,
    })?;
    let pins = pinned_of(pins, &text)?;
    let faults = problems(&pins, &labels_of(&reference)?);
    if !faults.is_empty() {
        return Err(ReleaseError::Invalid(format!(
            "{reference} does not say what its recipe pins:\n{}",
            faults.join("\n")
        )));
    }
    eprintln!("{reference} holds:");
    for (pin, version) in &pins {
        eprintln!("  {:<28} {version}", pin.label);
    }
    let digest = capture(
        Command::new("docker").args(["image", "inspect", "--format", "{{.Id}}", &reference]),
    )?;
    Ok(Image {
        name: name.to_owned(),
        reference,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn recipe_file(name: &str) -> String {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the repository");
        let path: PathBuf = root.join(RECIPE).join(name);
        fs::read_to_string(&path).expect("a file of the recipe")
    }

    fn labels_saying(pins: &[(Pin, String)]) -> BTreeMap<String, String> {
        pins.iter()
            .map(|(pin, version)| (pin.label.to_owned(), version.clone()))
            .collect()
    }

    #[test]
    fn every_version_the_recipe_pins_is_written_into_a_label_of_the_image() {
        let text = recipe_file(BUILD_FILE);
        pinned_of(&PINS, &text).expect("the shipped recipe pins every version");
        for pin in PINS {
            let written = format!("{}=\"${{{}}}\"", pin.label, pin.argument);
            assert!(
                text.contains(&written),
                "the recipe does not label {} with {}",
                pin.label,
                pin.argument
            );
        }
    }

    #[test]
    fn every_version_the_developer_recipe_pins_is_written_into_a_label_of_its_image() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the repository");
        let text = fs::read_to_string(root.join(DEVELOPER_RECIPE).join(BUILD_FILE))
            .expect("the developer recipe");
        let pins =
            pinned_of(&DEVELOPER_PINS, &text).expect("the shipped recipe pins every version");
        for pin in DEVELOPER_PINS {
            let written = format!("{}=\"${{{}}}\"", pin.label, pin.argument);
            assert!(
                text.contains(&written),
                "the recipe does not label {} with {}",
                pin.label,
                pin.argument
            );
        }
        let mut labels = labels_saying(&pins);
        assert!(problems(&pins, &labels).is_empty());
        labels.insert("orch.image.docker".to_owned(), "1.0.0".to_owned());
        assert_eq!(problems(&pins, &labels).len(), 1);
    }

    #[test]
    fn the_readme_lists_every_version_the_recipe_pins() {
        let pins = pinned_of(&PINS, &recipe_file(BUILD_FILE)).expect("the pins");
        let readme = recipe_file("README.md");
        for (pin, version) in pins {
            assert!(
                readme.contains(&version),
                "the README does not list {version}, which the recipe pins as {}",
                pin.argument
            );
        }
    }

    #[test]
    fn an_image_that_says_other_versions_than_the_recipe_pins_is_refused() {
        let pins = pinned_of(&PINS, &recipe_file(BUILD_FILE)).expect("the pins");
        let mut labels = labels_saying(&pins);
        assert!(problems(&pins, &labels).is_empty());
        labels.insert("orch.image.node".to_owned(), "1.0.0".to_owned());
        labels.remove("orch.image.openbox");
        let faults = problems(&pins, &labels);
        assert_eq!(faults.len(), 2, "{faults:?}");
        assert!(faults.iter().any(|fault| fault.contains("orch.image.node")));
        assert!(
            faults
                .iter()
                .any(|fault| fault.contains("no label orch.image.openbox"))
        );
    }

    #[test]
    fn a_recipe_that_leaves_a_version_unpinned_is_refused() {
        let text = recipe_file(BUILD_FILE).replace("ARG NODE_VERSION=", "ARG NODE_VERSION_UNSET=");
        let refusal = pinned_of(&PINS, &text).expect_err("a recipe without the pin");
        assert!(refusal.to_string().contains("orch.image.node"), "{refusal}");
    }

    #[test]
    fn only_a_name_that_can_be_a_tag_builds_an_image() {
        for good in ["dev", "0.1.0", "image-final", "1.2.3-rc.1_x"] {
            assert!(is_tag(good), "{good}");
        }
        for bad in [
            "",
            "-x",
            ".x",
            "a b",
            "a:b",
            "a/b",
            &"a".repeat(LONGEST_TAG + 1),
        ] {
            assert!(!is_tag(bad), "{bad}");
        }
    }
}
