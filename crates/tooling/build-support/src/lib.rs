//! Build-script support shared by the Orch workspace.
//!
//! A build is identified by the release it belongs to and by a build number
//! that tells two builds of one release apart. Both are fixed while a crate's
//! build script runs and reach the compiled code as environment variables
//! read with `env!`.
//!
//! # Release identity
//!
//! The version is, in order of preference: `ORCH_RELEASE_VERSION`; the
//! release tag without its leading `v`; the package version from
//! `Cargo.toml`. The tag is `ORCH_RELEASE_TAG`, or the tag GitHub Actions is
//! building (`GITHUB_REF_TYPE=tag` with `GITHUB_REF_NAME`). A version given
//! together with a tag must agree with it.
//!
//! # Build numbers
//!
//! Each separately shipped component has a counter file,
//! `build/build-numbers/<component>.txt`, holding one positive integer. A
//! local build advances the counter and takes the new value. A build inside
//! GitHub Actions takes the committed value as it is, so every job of one
//! workflow run agrees on the number. `ORCH_BUILD_NUMBER_DIR` names another
//! directory for builds that must leave the tracked counters alone.
//!
//! Counters use operating-system locks, released even if a build crashes.
//! Lock files remain in place so every waiter uses the same lock.

#![forbid(unsafe_code)]

use std::{
    env, fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Overrides the release version.
const RELEASE_VERSION_VARIABLE: &str = "ORCH_RELEASE_VERSION";
/// Names the release tag a build is made from.
const RELEASE_TAG_VARIABLE: &str = "ORCH_RELEASE_TAG";
/// Names a directory of counters to use instead of the tracked one.
const BUILD_NUMBER_DIR_VARIABLE: &str = "ORCH_BUILD_NUMBER_DIR";
/// Set to `true` by GitHub Actions in every job.
const GITHUB_ACTIONS_VARIABLE: &str = "GITHUB_ACTIONS";
/// `tag` when GitHub Actions builds a tag.
const GITHUB_REF_TYPE_VARIABLE: &str = "GITHUB_REF_TYPE";
/// The name of the branch or tag GitHub Actions builds.
const GITHUB_REF_NAME_VARIABLE: &str = "GITHUB_REF_NAME";

/// The release a build belongs to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseIdentity {
    /// `MAJOR.MINOR.PATCH`, with a pre-release suffix when there is one.
    pub version: String,
    /// The Git tag the build was made from, when it was made from one.
    pub tag: Option<String>,
}

/// Why the identity of a build could not be established.
#[derive(Debug)]
pub enum Error {
    /// Cargo did not provide a variable that every build script receives.
    MissingCargoVariable(&'static str),
    /// A variable holds a value that is not valid Unicode.
    NotUnicode(&'static str),
    /// The release version or tag is malformed, or the two disagree.
    Release(String),
    /// The component name cannot name a counter file and a variable.
    Component(String),
    /// The directory given for the counters is not an absolute path.
    CounterDir(PathBuf),
    /// A counter file holds something other than one positive integer, or
    /// cannot be advanced.
    Counter {
        /// The counter file.
        path: PathBuf,
        /// What is wrong with it, completing `counter <path> …`.
        problem: &'static str,
    },
    /// A file operation on a counter or its lock failed.
    Io {
        /// What was being done, completing "cannot …".
        action: &'static str,
        /// The file it was being done to.
        path: PathBuf,
        /// The failure.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCargoVariable(name) => {
                write!(formatter, "{name} is not set; run this through Cargo")
            }
            Self::NotUnicode(name) => write!(formatter, "{name} is not valid Unicode"),
            Self::Release(problem) => formatter.write_str(problem),
            Self::Component(name) => write!(
                formatter,
                "component name {name:?} must be one or more lowercase ASCII letters"
            ),
            Self::CounterDir(path) => write!(
                formatter,
                "{BUILD_NUMBER_DIR_VARIABLE} must be an absolute path, got {}",
                path.display()
            ),
            Self::Counter { path, problem } => {
                write!(formatter, "build counter {} {problem}", path.display())
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Resolves the release identity and gives it to the crate being built as
/// `ORCH_RELEASE_VERSION` and `ORCH_RELEASE_TAG`. The tag is empty when the
/// build is not made from one, so the compiled value never depends on what
/// the variable holds in the developer's shell.
///
/// # Errors
///
/// Fails when the version or tag is malformed, when the two disagree, or
/// when the function is called outside a Cargo build script.
pub fn emit_release_identity() -> Result<ReleaseIdentity, Error> {
    for name in [
        RELEASE_VERSION_VARIABLE,
        RELEASE_TAG_VARIABLE,
        GITHUB_REF_TYPE_VARIABLE,
        GITHUB_REF_NAME_VARIABLE,
    ] {
        println!("cargo::rerun-if-env-changed={name}");
    }

    let package_version = read_variable("CARGO_PKG_VERSION")?
        .ok_or(Error::MissingCargoVariable("CARGO_PKG_VERSION"))?;
    let identity = release_identity(&package_version)?;

    println!(
        "cargo::rustc-env={RELEASE_VERSION_VARIABLE}={}",
        identity.version
    );
    println!(
        "cargo::rustc-env={RELEASE_TAG_VARIABLE}={}",
        identity.tag.as_deref().unwrap_or_default()
    );
    Ok(identity)
}

/// The release identity the environment states, by the rules of
/// [`emit_release_identity`], for a tool that is not a build script (the
/// packaging command asks it for a component Cargo does not build): Cargo is
/// told nothing. `package_version` is what the version is when neither the
/// version nor a tag is given.
///
/// # Errors
///
/// Fails when the version or tag is malformed, or when the two disagree.
pub fn release_identity(package_version: &str) -> Result<ReleaseIdentity, Error> {
    let inputs = ReleaseInputs {
        version: read_variable(RELEASE_VERSION_VARIABLE)?,
        tag: read_variable(RELEASE_TAG_VARIABLE)?,
        github_ref_type: read_variable(GITHUB_REF_TYPE_VARIABLE)?,
        github_ref_name: read_variable(GITHUB_REF_NAME_VARIABLE)?,
    };
    resolve_release_identity(inputs, package_version)
}

/// Takes the build number of `component`, as [`emit_component_build_number`]
/// does, without telling Cargo: a local build advances the counter, and a
/// build inside GitHub Actions reads the committed one.
///
/// # Errors
///
/// The same as [`emit_component_build_number`].
pub fn take_build_number(component: &str) -> Result<u64, Error> {
    if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err(Error::Component(component.to_owned()));
    }
    let file = format!("{component}.txt");
    if in_github_actions()? {
        // The committed counter, wherever ORCH_BUILD_NUMBER_DIR points the local ones: a release job's own
        // counter directory starts empty.
        read_counter(&committed_counter_dir()?.join(file))
    } else {
        advance_counter(&counter_dir()?.join(file))
    }
}

/// Takes the build number of `component` and gives it to the crate being
/// built as `ORCH_<COMPONENT>_BUILD_NUMBER`.
///
/// A local build advances the counter first. A build inside GitHub Actions
/// reads it without changing it.
///
/// # Errors
///
/// Fails when the component name is not lowercase ASCII letters, when the
/// counter is missing inside GitHub Actions, when it does not hold one
/// positive integer, or when it cannot be read, locked or written.
pub fn emit_component_build_number(component: &str) -> Result<u64, Error> {
    let build_number = take_build_number(component)?;

    println!("cargo::rerun-if-env-changed={BUILD_NUMBER_DIR_VARIABLE}");
    println!("cargo::rerun-if-env-changed={GITHUB_ACTIONS_VARIABLE}");
    println!(
        "cargo::rerun-if-changed={}",
        counter_dir()?.join(format!("{component}.txt")).display()
    );
    println!(
        "cargo::rustc-env=ORCH_{}_BUILD_NUMBER={build_number}",
        component.to_ascii_uppercase()
    );
    Ok(build_number)
}

/// What the environment says about the release, before any of it is checked.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ReleaseInputs {
    version: Option<String>,
    tag: Option<String>,
    github_ref_type: Option<String>,
    github_ref_name: Option<String>,
}

fn resolve_release_identity(
    inputs: ReleaseInputs,
    package_version: &str,
) -> Result<ReleaseIdentity, Error> {
    let github_tag = match (inputs.github_ref_type.as_deref(), inputs.github_ref_name) {
        (Some("tag"), Some(name)) => Some(name),
        _ => None,
    };
    let tag = inputs.tag.or(github_tag);

    let tag_version = match tag.as_deref() {
        None => None,
        Some(tag) => match tag.strip_prefix('v') {
            Some(version) if is_release_version(version) => Some(version.to_owned()),
            _ => {
                return Err(Error::Release(format!(
                    "release tag {tag:?} must be `v` followed by a canonical version, such as v1.2.3 or v1.2.3-rc.1"
                )));
            }
        },
    };

    let version = match (inputs.version, tag_version) {
        (Some(version), Some(tag_version)) if version != tag_version => {
            return Err(Error::Release(format!(
                "{RELEASE_VERSION_VARIABLE} is {version:?} but the release tag says {tag_version:?}"
            )));
        }
        (Some(version), _) => version,
        (None, Some(tag_version)) => tag_version,
        (None, None) => package_version.trim().to_owned(),
    };
    if !is_release_version(&version) {
        return Err(Error::Release(format!(
            "release version {version:?} must be a canonical version, such as 1.2.3 or 1.2.3-rc.1"
        )));
    }

    Ok(ReleaseIdentity { version, tag })
}

/// Whether `version` is `MAJOR.MINOR.PATCH` with an optional pre-release
/// suffix, in the canonical form of Semantic Versioning 2.0.0. Build
/// metadata is refused: a release is identified by its tag alone.
fn is_release_version(version: &str) -> bool {
    let (core, prerelease) = match version.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (version, None),
    };
    let mut numbers = core.split('.');
    let core_is_three_numbers =
        (0..3).all(|_| numbers.next().is_some_and(is_canonical_number)) && numbers.next().is_none();
    let prerelease_is_identifiers =
        prerelease.is_none_or(|prerelease| prerelease.split('.').all(is_prerelease_identifier));
    core_is_three_numbers && prerelease_is_identifiers
}

/// Decimal digits without a leading zero, or zero itself.
fn is_canonical_number(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'))
}

/// ASCII letters, digits and hyphens; when all digits, a canonical number.
fn is_prerelease_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && (!text.bytes().all(|byte| byte.is_ascii_digit()) || is_canonical_number(text))
}

/// Reads a variable, treating an empty or blank value as unset.
fn read_variable(name: &'static str) -> Result<Option<String>, Error> {
    match env::var(name) {
        Ok(value) => {
            let value = value.trim();
            Ok((!value.is_empty()).then(|| value.to_owned()))
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(Error::NotUnicode(name)),
    }
}

fn in_github_actions() -> Result<bool, Error> {
    Ok(read_variable(GITHUB_ACTIONS_VARIABLE)?
        .is_some_and(|value| value.eq_ignore_ascii_case("true")))
}

/// The directory holding the counters: the one named by the environment, or
/// `build/build-numbers` at the workspace root, independent of the CWD.
fn counter_dir() -> Result<PathBuf, Error> {
    if let Some(dir) = read_variable(BUILD_NUMBER_DIR_VARIABLE)? {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(Error::CounterDir(dir))
        };
    }
    committed_counter_dir()
}

/// The counters committed with the source, `build/build-numbers` of the workspace.
fn committed_counter_dir() -> Result<PathBuf, Error> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .ok_or(Error::MissingCargoVariable("CARGO_MANIFEST_DIR"))?;
    Ok(workspace.join("build/build-numbers"))
}

fn read_counter(path: &Path) -> Result<u64, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        action: "read the build counter",
        path: path.to_owned(),
        source,
    })?;
    parse_counter(&text).ok_or_else(|| Error::Counter {
        path: path.to_owned(),
        problem: "must contain one positive integer",
    })
}

/// One positive decimal integer without a sign or leading zero, surrounded
/// by nothing but white space.
fn parse_counter(text: &str) -> Option<u64> {
    let digits = text.trim();
    if digits.is_empty()
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

/// Adds one to the counter under its lock and returns the new value. A
/// counter that does not exist yet starts at one.
fn advance_counter(path: &Path) -> Result<u64, Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|source| Error::Io {
            action: "create the build counter directory",
            path: dir.to_owned(),
            source,
        })?;
    }

    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| Error::Io {
            action: "open the build counter lock",
            path: lock_path.clone(),
            source,
        })?;
    lock.lock().map_err(|source| Error::Io {
        action: "take the build counter lock",
        path: lock_path,
        source,
    })?;
    let current = match read_counter(path) {
        Ok(value) => value,
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let next = current.checked_add(1).ok_or_else(|| Error::Counter {
        path: path.to_owned(),
        problem: "is exhausted",
    })?;

    // Written beside the counter and moved over it, so that a build which
    // dies here leaves the previous value and never an empty file.
    let staging = path.with_extension("tmp");
    let write_staging = || -> io::Result<()> {
        let mut file = fs::File::create(&staging)?;
        writeln!(file, "{next}")?;
        file.sync_all()
    };
    write_staging().map_err(|source| Error::Io {
        action: "write the build counter",
        path: staging.clone(),
        source,
    })?;
    fs::rename(&staging, path).map_err(|source| Error::Io {
        action: "replace the build counter",
        path: path.to_owned(),
        source,
    })?;
    Ok(next)
}
