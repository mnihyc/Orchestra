//! A skill's files fetched onto the host for the server, which fetches no address itself (`node.skill.fetch`).
//!
//! A Git repository is read the way an image's build context is: by a Git that asks nothing of anybody (no
//! prompt, no password helper, no system or global configuration), only over schemes that need no
//! credential, at the commit its branch or tag names now, one commit deep, and the folder asked for is
//! written as a tar by Git itself. An archive at an address is downloaded as it is. Either is put up for the
//! command as one blob, refused when it is larger than the command allows, and checked by the server, which
//! reads the skill out of it; nothing of it is run here.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_connector_core::content::Content;
use orch_connector_core::roles::Keys;
use orch_link_contract::commands::{
    CommandError, ErrorCode, NodeSkillFetch, NodeSkillFetchResult, Secret, SecretOrText,
    SkillFetchSource,
};
use orch_notifier_contract::outbound;
use orch_types::Id;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// The schemes a repository may be read over: those that need no credential, as for a build context; a file
/// address would read the host's own disk and an ssh one asks for a key.
const GIT_SCHEMES: [&str; 3] = ["https", "http", "git"];

/// How long one Git step may take: a shallow fetch of a skill's repository is seconds, and the command's own
/// deadline bounds the whole.
const GIT_STEP: Duration = Duration::from_secs(300);

/// How long the connection to an address may take to open, and how long its body may stand still.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Match the downloader's previous redirect limit while checking each destination anew.
const MAX_REDIRECTS: usize = 10;

/// How many hexadecimal digits a full commit name has.
const COMMIT_HEX: usize = 40;

/// The longest reference name taken: Git's own limit of a ref's name.
const REFERENCE_MAX: usize = 255;

/// The file the archive is written to, in the command's own directory.
const ARCHIVE: &str = "skill-archive";

/// Fetches skills for the node's commands.
#[derive(Debug, Clone)]
pub struct SkillFetch {
    policy: Option<Arc<orch_connector_core::policy::PolicyStore>>,
}

fn refused(code: ErrorCode, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn internal(what: &str, error: impl std::fmt::Display) -> CommandError {
    refused(ErrorCode::Internal, format!("{what}: {error}"))
}

/// Whether `reference` can be given to Git as a branch, a tag or a commit: no option, no space, no control
/// character, and not longer than a ref's name may be.
fn plain_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= REFERENCE_MAX
        && !reference.starts_with('-')
        && !reference.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == ':'
        })
}

/// Whether `folder` is a relative path inside a repository.
fn plain_folder(folder: &str) -> bool {
    !folder.starts_with('/')
        && !folder.starts_with('-')
        && !folder.chars().any(char::is_control)
        && folder
            .split('/')
            .all(|part| !part.is_empty() && part != "..")
}

impl SkillFetch {
    /// A fetcher held to the machine's policy, as its downloads are.
    pub fn new(
        policy: Option<Arc<orch_connector_core::policy::PolicyStore>>,
    ) -> Result<Self, CommandError> {
        Ok(Self { policy })
    }

    /// A host whose operator switched fetching off fetches nothing.
    fn require_fetch(&self) -> Result<(), CommandError> {
        self.policy
            .as_ref()
            .map_or(Ok(()), |policy| policy.require_tools_fetch())
    }

    /// Carries out `node.skill.fetch` for `command`.
    pub async fn fetch(
        &self,
        content: &Content,
        keys: &Keys,
        command: Id,
        body: &NodeSkillFetch,
        cancel: &CancellationToken,
    ) -> Result<NodeSkillFetchResult, CommandError> {
        self.require_fetch()?;
        let max = u64::try_from(body.max_bytes)
            .map_err(|_| refused(ErrorCode::Unsupported, "a negative limit takes nothing"))?;
        let directory = content.directory_of(command);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| internal("no room for the archive", error))?;
        let archive = directory.join(ARCHIVE);
        let outcome = async {
            let commit = match &body.source {
                SkillFetchSource::Git {
                    repository,
                    reference,
                    folder,
                } => Some(
                    self.git(
                        &directory,
                        repository,
                        reference,
                        folder.as_deref(),
                        &archive,
                        cancel,
                    )
                    .await?,
                ),
                SkillFetchSource::Url { url } => {
                    let url = source_url(keys, command, url)?;
                    self.download(&url, &archive, max, &body.allowed_private_hosts, cancel)
                        .await?;
                    None
                }
            };
            let size = tokio::fs::metadata(&archive)
                .await
                .map_err(|error| internal("the archive was not written", error))?
                .len();
            if size > max {
                return Err(refused(
                    ErrorCode::TooLarge,
                    format!("the archive has {size} bytes; at most {max} are taken"),
                ));
            }
            let blob = content.put(command, &archive).await?;
            Ok(NodeSkillFetchResult {
                archive: blob,
                commit,
            })
        }
        .await;
        content.finish(command).await;
        outcome
    }

    /// The commit `reference` names in `repository`, its `folder` written as a tar to `archive`.
    async fn git(
        &self,
        scratch: &Path,
        repository: &str,
        reference: &str,
        folder: Option<&str>,
        archive: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, CommandError> {
        let scheme = reqwest::Url::parse(repository)
            .ok()
            .map(|url| url.scheme().to_owned());
        if !scheme
            .as_deref()
            .is_some_and(|scheme| GIT_SCHEMES.contains(&scheme))
        {
            return Err(refused(
                ErrorCode::Unsupported,
                format!("a repository is read over {} only", GIT_SCHEMES.join(", ")),
            ));
        }
        if !plain_reference(reference) {
            return Err(refused(
                ErrorCode::Unsupported,
                "the reference is no branch, tag or commit name",
            ));
        }
        if let Some(folder) = folder
            && !plain_folder(folder)
        {
            return Err(refused(
                ErrorCode::Unsupported,
                "the folder is no path inside the repository",
            ));
        }
        // A lookup must not discover the repository the node was started in.
        let work = tempfile::Builder::new()
            .prefix("skill-git-")
            .tempdir_in(scratch)
            .map_err(|error| internal("no room for the repository", error))?;
        git(&["init", "--quiet"], work.path(), cancel).await?;
        let commit = if reference.len() == COMMIT_HEX
            && reference.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            reference.to_ascii_lowercase()
        } else {
            let peeled = format!("{reference}^{{}}");
            let listed = git(
                &[
                    "ls-remote",
                    "--exit-code",
                    "--",
                    repository,
                    reference,
                    &peeled,
                ],
                work.path(),
                cancel,
            )
            .await
            .map_err(|error| {
                if error.code == ErrorCode::Internal.as_str() {
                    refused(
                        ErrorCode::NotFound,
                        format!("{reference} is not in the repository, or it cannot be read"),
                    )
                } else {
                    error
                }
            })?;
            commit_of(&listed, reference).ok_or_else(|| {
                refused(
                    ErrorCode::NotFound,
                    format!("{reference} is not in the repository"),
                )
            })?
        };
        git(
            &[
                "fetch", "--quiet", "--depth", "1", "--", repository, &commit,
            ],
            work.path(),
            cancel,
        )
        .await?;
        let tree = match folder {
            Some(folder) => format!("FETCH_HEAD:{folder}"),
            None => "FETCH_HEAD".to_owned(),
        };
        let output = archive.display().to_string();
        git(
            &["archive", "--format=tar", "-o", &output, &tree],
            work.path(),
            cancel,
        )
        .await?;
        Ok(commit)
    }

    /// Downloads `url` to `into`, stopping as soon as it is larger than `max`.
    async fn download(
        &self,
        url: &str,
        into: &Path,
        max: u64,
        allowed: &[String],
        cancel: &CancellationToken,
    ) -> Result<(), CommandError> {
        let mut url = reqwest::Url::parse(url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or_else(|| {
                refused(
                    ErrorCode::Unsupported,
                    "an archive is downloaded from an http or https address",
                )
            })?;
        // The address is told without its query: a signed address carries a secret there.
        let shown = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.path()
        );
        let mut reply = None;
        for hop in 0..=MAX_REDIRECTS {
            let client = checked_client(&url, allowed, cancel).await?;
            let response = tokio::select! {
                response = client.get(url.clone()).send() => response
                    .map_err(|error| refused(ErrorCode::NotFound, format!("{shown}: {}", error.without_url())))?,
                () = cancel.cancelled() => return Err(refused(ErrorCode::Timeout, "the fetch was stopped")),
            };
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == MAX_REDIRECTS {
                    return Err(refused(
                        ErrorCode::Unsupported,
                        "the source redirected too many times",
                    ));
                }
                url = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| url.join(location).ok())
                    .ok_or_else(|| {
                        refused(
                            ErrorCode::Unsupported,
                            "the source gave an invalid redirect",
                        )
                    })?;
                continue;
            }
            reply = Some(response);
            break;
        }
        let reply = reply.ok_or_else(|| {
            refused(
                ErrorCode::Unsupported,
                "the source redirected too many times",
            )
        })?;
        if !reply.status().is_success() {
            return Err(refused(
                ErrorCode::NotFound,
                format!("{shown} answered {}", reply.status()),
            ));
        }
        let mut file = tokio::fs::File::create(into)
            .await
            .map_err(|error| internal("the download file cannot be made", error))?;
        let mut size: u64 = 0;
        let mut body = reply.bytes_stream();
        loop {
            let piece = tokio::select! {
                piece = body.next() => piece,
                () = cancel.cancelled() => return Err(refused(ErrorCode::Timeout, "the fetch was stopped")),
            };
            let Some(piece) = piece else { break };
            let piece = piece.map_err(|error| {
                refused(
                    ErrorCode::NotFound,
                    format!("{shown}: {}", error.without_url()),
                )
            })?;
            size = size.saturating_add(piece.len() as u64);
            if size > max {
                return Err(refused(
                    ErrorCode::TooLarge,
                    format!("{shown} holds more than the {max} bytes taken"),
                ));
            }
            file.write_all(&piece)
                .await
                .map_err(|error| internal("the download cannot be written", error))?;
        }
        file.flush()
            .await
            .map_err(|error| internal("the download cannot be written", error))
    }
}

/// Resolve, check and pin the address actually used for one request. Proxies would resolve it again.
async fn checked_client(
    url: &reqwest::Url,
    allowed: &[String],
    cancel: &CancellationToken,
) -> Result<reqwest::Client, CommandError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(refused(
            ErrorCode::Unsupported,
            "an archive is downloaded from an http or https address",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| refused(ErrorCode::Unsupported, "the source names no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url
        .port_or_known_default()
        .ok_or_else(|| refused(ErrorCode::Unsupported, "the source names no port"))?;
    let addresses: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::select! {
            result = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port))) => result
                .map_err(|_| refused(ErrorCode::Timeout, "the source host lookup timed out"))?
                .map_err(|_| refused(ErrorCode::NotFound, "the source host cannot be looked up"))?
                .collect(),
            () = cancel.cancelled() => return Err(refused(ErrorCode::Timeout, "the fetch was stopped")),
        }
    };
    let vetted: Vec<_> = addresses
        .into_iter()
        .filter(|address| outbound::may_connect(host, address.ip(), Some(allowed)).is_ok())
        .collect();
    if vetted.is_empty() {
        return Err(refused(
            ErrorCode::Unsupported,
            "the source address is not allowed or has no usable address",
        ));
    }
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .resolve_to_addrs(host, &vetted)
        .build()
        .map_err(|error| internal("the downloader cannot be made", error.without_url()))
}

/// A command may store a public address or a reference to a skill-owned secret, never a URL
/// with credentials in plain text. The link replaces the reference with a value sealed to this
/// machine and this command before sending it.
fn source_url(keys: &Keys, command: Id, url: &SecretOrText) -> Result<String, CommandError> {
    match url {
        SecretOrText::Text(url) => {
            if reqwest::Url::parse(url).ok().is_some_and(|parsed| {
                !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
            }) {
                return Err(refused(
                    ErrorCode::Unsupported,
                    "a private source address must arrive sealed to this machine",
                ));
            }
            Ok(url.clone())
        }
        SecretOrText::Secret(Secret::Sealed(value)) => keys.open(command, value),
        SecretOrText::Secret(Secret::Ref(_)) => Err(refused(
            ErrorCode::Unsupported,
            "a source address still names a stored secret",
        )),
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use orch_pki::ca::{CertificateAuthority, Holder};
    use orch_pki::sealing::{RecipientKey, info_for_command, seal};
    use orch_types::{ManualClock, new_id};

    #[test]
    fn a_private_source_opens_only_from_a_sealed_delivery_to_its_command() {
        let clock = ManualClock::new("2026-10-11T12:00:00Z".parse().unwrap());
        let authority = CertificateAuthority::create("Test", 3650, &clock).unwrap();
        let (key, certificate) = authority
            .issue_with_new_key(&Holder::Machine(new_id()), 365, &clock)
            .unwrap();
        let keys = Keys::new(key);
        let command = new_id();
        for private in [
            "https://example.invalid/skill.tar.gz?token=sample-secret",
            "https://alice:sample-secret@example.invalid/skill.tar.gz",
        ] {
            let sealed = seal(
                &RecipientKey::from_certificate(&certificate),
                &info_for_command(command),
                private.as_bytes(),
            )
            .unwrap();
            let delivered = SecretOrText::Secret(Secret::Sealed(sealed));
            assert_eq!(source_url(&keys, command, &delivered).unwrap(), private);
            assert!(source_url(&keys, new_id(), &delivered).is_err());
            assert!(source_url(&keys, command, &SecretOrText::Text(private.into())).is_err());
        }
        assert!(source_url(&keys, command, &SecretOrText::Secret(Secret::Ref(new_id()))).is_err());
    }
}

/// The commit `ls-remote` gave for `reference`: the commit a tag points at where it is an annotated one.
fn commit_of(listed: &str, reference: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in listed.lines() {
        let Some((commit, name)) = line.split_once('\t') else {
            continue;
        };
        if commit.len() != COMMIT_HEX || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        if name.ends_with("^{}") {
            return Some(commit.to_owned());
        }
        if found.is_none() || name.ends_with(&format!("/{reference}")) {
            found = Some(commit.to_owned());
        }
    }
    found
}

fn git_command(arguments: &[&str], directory: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.templateDir=",
        ])
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_ALLOW_PROTOCOL", GIT_SCHEMES.join(":"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    // Keep the operator's network and certificate settings, but never let an
    // inherited repository selection replace this scratch repo.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_TEMPLATE_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

/// Runs Git with nothing to ask anybody: no terminal prompt, no password helper, no system or global
/// configuration, only the schemes that need no credential, and a step's time.
async fn git(
    arguments: &[&str],
    directory: &Path,
    cancel: &CancellationToken,
) -> Result<String, CommandError> {
    let mut command = git_command(arguments, directory);
    let run = command.output();
    let output = tokio::select! {
        output = tokio::time::timeout(GIT_STEP, run) => output
            .map_err(|_| refused(ErrorCode::Timeout, format!("git {} took too long", arguments.first().copied().unwrap_or_default())))?
            .map_err(|error| refused(ErrorCode::Unsupported, format!("git cannot be run here: {error}")))?,
        () = cancel.cancelled() => return Err(refused(ErrorCode::Timeout, "the fetch was stopped")),
    };
    if !output.status.success() {
        if matches!(arguments.first().copied(), Some("ls-remote" | "fetch")) {
            return Err(internal("git", "the repository could not be read"));
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let last = said.lines().last().unwrap_or_default().trim().to_owned();
        return Err(internal(
            &format!("git {}", arguments.first().copied().unwrap_or_default()),
            last,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, http::StatusCode, routing::get};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_skill_archive_redirect_cannot_reach_an_unlisted_private_host() {
        let mut listener = None;
        for port in 41000..=41999 {
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.expect("a test port is available");
        let port = listener.local_addr().unwrap().port();
        let reached = Arc::new(AtomicUsize::new(0));
        let target = format!("http://127.0.0.1:{port}/archive");
        let app = Router::new()
            .route(
                "/redirect",
                get(move || {
                    let target = target.clone();
                    async move { (StatusCode::FOUND, [("location", target)]) }
                }),
            )
            .route(
                "/archive",
                get(|State(reached): State<Arc<AtomicUsize>>| async move {
                    reached.fetch_add(1, Ordering::SeqCst);
                    "archive bytes"
                }),
            )
            .with_state(Arc::clone(&reached));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive");
        let fetcher = SkillFetch::new(None).unwrap();
        let source = format!("http://localhost:{port}/redirect");
        let cancel = CancellationToken::new();
        let allowed = vec!["localhost".to_owned()];
        assert!(
            fetcher
                .download(&source, &path, 1024, &allowed, &cancel)
                .await
                .is_err()
        );
        assert_eq!(
            reached.load(Ordering::SeqCst),
            0,
            "the private redirect was never contacted"
        );
        let allowed = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
        fetcher
            .download(&source, &path, 1024, &allowed, &cancel)
            .await
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"archive bytes");
        assert_eq!(reached.load(Ordering::SeqCst), 1);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn a_skill_archive_refuses_unlisted_internal_addresses_before_connecting() {
        let cancel = CancellationToken::new();
        for address in [
            "http://127.0.0.1:41000/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:127.0.0.1]:41000/",
            "file:///etc/passwd",
        ] {
            let url = reqwest::Url::parse(address).unwrap();
            assert!(
                checked_client(&url, &[], &cancel).await.is_err(),
                "{address}"
            );
        }
        let public = reqwest::Url::parse("https://8.8.8.8/archive").unwrap();
        assert!(checked_client(&public, &[], &cancel).await.is_ok());
    }

    #[tokio::test]
    async fn a_git_skill_source_does_not_expose_a_remote_service_response_as_an_error() {
        let mut listener = None;
        for port in 41000..=41999 {
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.expect("a test port is available");
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().fallback(get(|| async {
            (
                [(
                    "content-type",
                    "application/x-git-upload-pack-advertisement",
                )],
                "secret-response-from-private-service",
            )
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("http://127.0.0.1:{port}/repo");
        let error = git(
            &["ls-remote", "--exit-code", "--", &url, "main"],
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(error.detail.contains("the repository could not be read"));
        assert!(!error.detail.contains("secr"));
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn a_skill_lookup_uses_its_own_repository_inside_another_repository() {
        let parent = tempfile::tempdir().unwrap();
        let outer = parent.path().join(".git");
        std::fs::create_dir_all(outer.join("objects")).unwrap();
        std::fs::create_dir_all(outer.join("refs/heads")).unwrap();
        std::fs::write(outer.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            outer.join("config"),
            "[core]\nrepositoryformatversion = 0\nbare = false\n",
        )
        .unwrap();
        let work = parent.path().join("private");
        std::fs::create_dir(&work).unwrap();
        let cancel = CancellationToken::new();
        git(&["init", "--quiet"], &work, &cancel).await.unwrap();
        let top = git(&["rev-parse", "--show-toplevel"], &work, &cancel)
            .await
            .unwrap();
        assert_eq!(
            std::fs::canonicalize(top.trim()).unwrap(),
            std::fs::canonicalize(work).unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_local_repository_cannot_run_a_credential_helper_during_a_skill_fetch() {
        let directory = tempfile::tempdir().unwrap();
        let git_dir = directory.path().join(".git");
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();
        std::fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[core]\nrepositoryformatversion = 0\nbare = false\n[credential]\nhelper = !printf hit > helper-ran\n",
        )
        .unwrap();
        let mut command = git_command(&["credential", "fill"], directory.path());
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", "!printf hit > injected-helper-ran");
        command.stdin(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(b"protocol=https\nhost=example.invalid\n\n")
            .await
            .unwrap();
        drop(input);
        let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(!directory.path().join("helper-ran").exists());
        assert!(!directory.path().join("injected-helper-ran").exists());
    }

    #[test]
    fn a_reference_or_a_folder_that_could_be_read_as_an_option_is_refused() {
        assert!(plain_reference("main"));
        assert!(plain_reference("v1.2.3"));
        assert!(plain_reference("releases/2026-10"));
        assert!(!plain_reference("--upload-pack=evil"));
        assert!(!plain_reference("main branch"));
        assert!(!plain_reference(""));
        assert!(plain_folder("skills/pdf"));
        assert!(!plain_folder("../outside"));
        assert!(!plain_folder("/etc"));
        assert!(!plain_folder("-x"));
    }

    #[test]
    fn an_annotated_tag_is_taken_at_the_commit_it_points_at() {
        let listed = "1111111111111111111111111111111111111111\trefs/tags/v1\n2222222222222222222222222222222222222222\trefs/tags/v1^{}\n";
        assert_eq!(
            commit_of(listed, "v1").as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
        let branch = "3333333333333333333333333333333333333333\trefs/heads/main\n";
        assert_eq!(
            commit_of(branch, "main").as_deref(),
            Some("3333333333333333333333333333333333333333")
        );
        assert_eq!(commit_of("garbage", "main"), None);
    }
}
