//! Native Git bytes/receipts behind the real link, with manually claimed engine attempts.
//! The sole transport substitution maps the synthetic HTTPS fixture to a local bare repository.

use super::*;
use crate::commands::{Commands, CommandsError, Info, Outcome};
use orch_envhost::{
    environment::{GateState, Gates, Limits},
    file_states::FileStates,
    files::Files,
    git::GitTool,
    ignore::IgnoreRules,
    index::{HashCache, Index},
    paths::{PathStyle, Workspace},
    state_store::ObjectStore,
};
use orch_link_contract::commands::{Command, CommandError};
use orch_link_contract::streams::Stream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

const BINARY: &[u8] = b"whole\0native\xffbytes\n";
const URL: &str = "https://example.test/team/repo.git";

fn completed<T>(result: Result<T, Stop>) -> TestResult<T> {
    result.map_err(|stop| format!("the source proof stopped: {stop:?}").into())
}

struct Native {
    _directory: tempfile::TempDir,
    files: Files,
    states: FileStates,
    tool: GitTool,
    real_git: PathBuf,
    generation: i64,
    clock: Arc<ManualClock>,
}

fn shell_word(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

impl Native {
    fn new(generation: i32, clock: Arc<ManualClock>) -> TestResult<Arc<Self>> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        let workspace = directory.path().join("workspace");
        let repository = workspace.join("repo");
        std::fs::create_dir_all(&repository)?;
        let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|path| path.join("git"))
            .find(|path| path.is_file())
            .ok_or("native Git is required")?;
        let files = Files::workspace(
            Workspace::open(&workspace, &[], PathStyle::Posix, true)?,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let states = FileStates::new(ObjectStore::open(
            &directory.path().join("kept"),
            &workspace,
        )?);
        let git = |cwd: &Path, arguments: &[&str]| Self::git_at(&real_git, cwd, arguments);
        git(&repository, &["init", "-b", "main"])?;
        std::fs::write(repository.join("code"), b"committed source\n")?;
        std::fs::write(repository.join("binary"), BINARY)?;
        git(&repository, &["add", "."])?;
        git(
            &repository,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@orch.invalid",
                "commit",
                "-m",
                "base",
            ],
        )?;
        let bare = directory.path().join("remote.git");
        git(
            directory.path(),
            &[
                "clone",
                "--bare",
                repository.to_str().ok_or("fixture path is not UTF-8")?,
                bare.to_str().ok_or("fixture path is not UTF-8")?,
            ],
        )?;
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin)?;
        let executable = bin.join("git");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nis_clone=no\nfor argument in \"$@\"; do\n [ \"$argument\" = clone ] && is_clone=yes\ndone\n\
            if [ \"$is_clone\" = yes ]; then\n {real} -c core.hooksPath=/dev/null clone --no-checkout --origin origin {bare} . || exit $?\n\
            {real} config remote.origin.url {URL} || exit $?\n exit 0\nfi\nexec {real} \"$@\"\n",
                real = shell_word(&real_git),
                bare = shell_word(&bare)
            ),
        )?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let tool = GitTool::find_in(&bin.into_os_string());
        Ok(Arc::new(Self {
            _directory: directory,
            files,
            states,
            tool,
            real_git,
            generation: i64::from(generation),
            clock,
        }))
    }

    fn git_at(program: &Path, cwd: &Path, arguments: &[&str]) -> TestResult<Vec<u8>> {
        let output = std::process::Command::new(program)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(arguments)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "synthetic Git fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(output.stdout)
    }

    fn git(&self, directory: &str, arguments: &[&str]) -> TestResult<Vec<u8>> {
        Self::git_at(
            &self.real_git,
            &self.files.root().root_path().join(directory),
            arguments,
        )
    }

    fn head(&self) -> TestResult<String> {
        Ok(
            String::from_utf8(self.git("repo", &["rev-parse", "HEAD"])?)?
                .trim()
                .to_owned(),
        )
    }

    fn read(&self, directory: &str, name: &str) -> TestResult<Vec<u8>> {
        Ok(std::fs::read(
            self.files.root().root_path().join(directory).join(name),
        )?)
    }

    fn branch_exists(&self, branch: &str) -> TestResult<bool> {
        let output = std::process::Command::new(&self.real_git)
            .current_dir(self.files.root().root_path().join("repo"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()?;
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "show-ref must answer existence"
        );
        Ok(output.status.success())
    }

    fn install(self: &Arc<Self>, fixture: &Fixture) {
        for kind in [
            CommandKind::FsStat,
            CommandKind::GitPrepare,
            CommandKind::GitPrepareCleanup,
            CommandKind::GitWorktree,
            CommandKind::GitWorktreeCleanup,
        ] {
            let native = self.clone();
            fixture.world.machine.handler.on(kind, move |command| {
                // Scripts are synchronous; these tests use a multithread runtime so native
                // async Git runs without blocking its workers or changing the shared double.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(native.answer(command))
                })
            });
        }
    }

    async fn answer(&self, command: &Command) -> Result<CommandResult, CommandError> {
        let result = async {
            Ok(match command {
                Command::FsStat(body) => {
                    CommandResult::FsStat(self.files.stat(&self.files.parse(&body.path)?)?)
                }
                Command::GitPrepare(body) => CommandResult::GitPrepare(
                    orch_envhost::git_actions::prepare(
                        &self.tool,
                        &self.files,
                        &self.states,
                        body,
                        self.generation,
                        false,
                        None,
                    )
                    .await?,
                ),
                Command::GitPrepareCleanup(body) => CommandResult::GitPrepareCleanup(
                    orch_envhost::git_actions::cleanup(
                        &self.files,
                        &self.states,
                        &body.original,
                        self.generation,
                    )
                    .await?,
                ),
                Command::GitWorktree(body) => CommandResult::GitWorktree(
                    orch_envhost::git_worktrees::create(
                        &self.tool,
                        &self.files,
                        &self.states,
                        body,
                        self.generation,
                        Time::new(self.clock.now()),
                    )
                    .await?,
                ),
                Command::GitWorktreeCleanup(body) => CommandResult::GitWorktreeCleanup(
                    orch_envhost::git_worktrees::cleanup(
                        &self.tool,
                        &self.files,
                        &self.states,
                        &body.original,
                        self.generation,
                        Time::new(self.clock.now()),
                    )
                    .await?,
                ),
                _ => {
                    return Err(orch_envhost::error::HostError::Unsupported(
                        "no native source fixture handler".into(),
                    ));
                }
            })
        }
        .await;
        result.map_err(|error: orch_envhost::error::HostError| error.command_error())
    }

    fn record(&self, fixture: &Fixture, snapshot: Id) -> TestResult<FsRecordResult> {
        let index = Index::new(
            orch_link_contract::commands::IndexMode::Full,
            Limits::default().index_max_entries,
        );
        self.states.request(
            &self.files.parse("repo")?,
            orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
        );
        index.scan(
            self.files.root(),
            self.files.ignore().ok_or("fixture ignore rules absent")?,
        )?;
        self.states.refresh(&self.files, &index, None, true)?;
        let result = self.states.record(
            &self.files,
            &FsRecord {
                environment_id: fixture.world.environment,
                snapshot_id: snapshot,
                directory: "repo".into(),
                kind: RecordKind::Turn,
                record_days: 7,
                max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
            },
            self.generation,
            Time::new(self.clock.now()),
        )?;
        assert_eq!(result.state, RecordState::Complete);
        Ok(result)
    }
}

/// Counts issue calls, including duplicate IDs, without changing link/native ordering or results.
struct IssueTap {
    inner: Arc<dyn Commands>,
    cleanups: AtomicUsize,
    changed: Notify,
}

#[async_trait]
impl Commands for IssueTap {
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.inner.observe(command).await
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<crate::commands::RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if matches!(
            command.kind,
            CommandKind::GitWorktreeCleanup | CommandKind::GitPrepareCleanup
        ) {
            self.cleanups.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_waiters();
        }
        self.inner.issue(command).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.inner.status(id).await
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.inner.wait(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }
    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}

impl IssueTap {
    async fn cleanup_issues(&self, count: usize) -> TestResult {
        tokio::time::timeout(common::PATIENCE, async {
            loop {
                let changed = self.changed.notified();
                if self.cleanups.load(Ordering::SeqCst) >= count {
                    return;
                }
                changed.await;
            }
        })
        .await?;
        Ok(())
    }
}

async fn fixture() -> TestResult<(Fixture, Arc<Native>, Arc<IssueTap>)> {
    let mut fixture = Fixture::new().await?;
    let tap = Arc::new(IssueTap {
        inner: fixture.world.commands.as_engine(),
        cleanups: AtomicUsize::new(0),
        changed: Notify::new(),
    });
    fixture.engine = Engine::new(EngineParts {
        db: fixture.world.pool.clone(),
        clock: fixture.world.clock.clone(),
        config: common::config(),
        commands: tap.clone(),
        access: fixture.world.access.clone(),
        live: fixture.world.live.clone(),
    });
    let generation: i32 = fixture
        .world
        .scalar(
            "SELECT generation FROM environments WHERE id=$1",
            fixture.world.environment,
        )
        .await;
    let native = Native::new(generation, fixture.clock.clone())?;
    native.install(&fixture);
    Ok((fixture, native, tap))
}

async fn pending_input_only(fixture: &Fixture) -> TestResult {
    let mut conn = fixture.world.pool.acquire().await?;
    assert_eq!(
        threads::input(&mut conn, fixture.input.id).await?.as_ref(),
        Some(&fixture.input)
    );
    assert_eq!(
        threads::pending_inputs(&mut conn, fixture.thread).await?,
        vec![fixture.input.clone()]
    );
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM turns WHERE thread_id=$1), (SELECT count(*) FROM leases WHERE thread_id=$1)")
        .bind(fixture.thread).fetch_one(&mut *conn).await?;
    assert_eq!(counts, (0, 0));
    for kind in [
        CommandKind::FsMkdir,
        CommandKind::FsRecord,
        CommandKind::SessionOpen,
        CommandKind::LeaseOpen,
        CommandKind::TurnStart,
    ] {
        assert_eq!(
            fixture.world.began(kind),
            0,
            "source recovery must not start {kind}"
        );
    }
    Ok(())
}

async fn applied_checkpoint(job: &Job, preparing: &Preparing) -> TestResult {
    assert_eq!(preparing.run(job, "validate").await, Ok(Done::Finished));
    assert_eq!(preparing.run(job, "apply").await, Ok(Done::Finished));
    job.amend(|progress, _| {
        for phase in &mut progress.phases {
            if matches!(phase.name.as_str(), "validate" | "apply") {
                phase.state = orch_api_contract::workspace::PhaseState::Done;
            }
        }
        progress.phase = "verify".into();
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_abort_survives_real_worktree_cleanup_takeover_and_explicit_retry() -> TestResult {
    let (fixture, native, tap) = fixture().await?;
    let mut plan = fixture.plan(true).await?;
    let Action::Worktree { body, .. } = &mut plan.action else {
        return Err("worktree plan absent".into());
    };
    body.choice.from = WorktreeFrom::Commit {
        commit: native.head()?,
    };
    let branch = body.choice.branch.clone();
    let directory = body.choice.directory.clone();
    let original_index = native.read("repo", ".git/index")?;
    let (old, preparing, _) = fixture.begin(plan).await?;
    applied_checkpoint(&old, &preparing).await?;
    assert_eq!(native.read(&directory, "binary")?, BINARY);
    assert_eq!(native.read(&directory, "code")?, b"committed source\n");
    assert_eq!(
        String::from_utf8(native.git(&directory, &["symbolic-ref", "--short", "HEAD"])?)?.trim(),
        branch
    );
    assert_eq!(
        native.git(&directory, &["rev-parse", "HEAD"])?,
        native.git("repo", &["rev-parse", "HEAD"])?
    );
    assert!(native.branch_exists(&branch)?);
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(fixture.world.environment)
        .execute(fixture.world.pool.pool())
        .await?;
    let Err(reason) = preparing.run(&old, "verify").await else {
        return Err("verification ignored the current write refusal".into());
    };
    assert!(
        matches!(&reason, Stop::Failed(failure) if failure.code == "workspace.remote_write_disabled")
    );
    fixture
        .world
        .machine
        .handler
        .hold(CommandKind::GitWorktreeCleanup);
    let engine = fixture.engine.clone();
    let ending = old.clone();
    let phase = preparing.clone();
    let stop = reason.clone();
    let first = tokio::spawn(async move { engine.end_job(&ending, Some(&phase), stop).await });
    fixture
        .world
        .wait_began(CommandKind::GitWorktreeCleanup, 1)
        .await;
    let intent = old.row().await?.plan["abort"].clone();
    assert!(
        !intent.is_null(),
        "cleanup admission follows the durable intent"
    );
    let current = fixture.take_over(old.id()).await?;
    sqlx::query("UPDATE environments SET remote_write=true WHERE id=$1")
        .bind(fixture.world.environment)
        .execute(fixture.world.pool.pool())
        .await?;
    assert_eq!(current.pass_commit().await, Err(reason.clone()));
    assert_eq!(preparing.publish(&current).await, Err(reason.clone()));
    let retained = old.row().await?.plan["file_result"].clone();
    fixture.engine.finish_job(&current, Some(&retained)).await?;
    assert_eq!(current.row().await?.state, state::RUNNING);
    let engine = fixture.engine.clone();
    let recovering = current.clone();
    let recovery = tokio::spawn(async move { engine.drive_job(&recovering).await });
    tap.cleanup_issues(2).await?;
    assert_eq!(
        fixture.world.began(CommandKind::GitWorktree),
        1,
        "takeover never resumes successful apply"
    );
    assert_eq!(fixture.source().await?["info"]["state"], "preparing");
    assert_eq!(native.read(&directory, "binary")?, BINARY);
    assert!(native.branch_exists(&branch)?);
    pending_input_only(&fixture).await?;
    fixture.world.machine.handler.release();
    tokio::time::timeout(common::PATIENCE, first).await???;
    tokio::time::timeout(common::PATIENCE, recovery).await???;
    let failed = current.row().await?;
    assert_eq!(failed.state, state::FAILED);
    assert_eq!(failed.plan["abort"], intent);
    assert_eq!(
        failed.error.as_ref().and_then(|error| error.get("code")),
        Some(&json!("workspace.remote_write_disabled"))
    );
    assert!(failed.result.is_none());
    assert_eq!(
        fixture.world.began(CommandKind::GitWorktreeCleanup),
        1,
        "both owners reconcile one retained cleanup command"
    );
    assert!(!native.files.root().root_path().join(&directory).exists());
    assert!(!native.branch_exists(&branch)?);
    assert_eq!(native.read("repo", ".git/index")?, original_index);
    assert_eq!(native.read("repo", "binary")?, BINARY);
    let source = fixture.source().await?;
    assert_eq!(source["info"]["state"], "failed");
    assert!(source["native_result"].is_null());
    let retry_plan: Plan = serde_json::from_value(source["native_plan"].clone())?;
    let Action::Worktree { body, .. } = &retry_plan.action else {
        return Err("retry worktree absent".into());
    };
    assert!(
        body.job_id.is_nil(),
        "completed removal requires a fresh native effect"
    );
    assert!(body.snapshot_id.is_none());
    pending_input_only(&fixture).await?;
    let (retry, retried, _) = fixture.begin(retry_plan).await?;
    let Action::Worktree { body: new_body, .. } = &retried.plan.action else {
        return Err("new worktree absent".into());
    };
    let Action::Worktree { body: old_body, .. } = &preparing.plan.action else {
        return Err("original worktree absent".into());
    };
    assert_ne!(new_body.job_id, old_body.job_id);
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&retry)).await??;
    assert_eq!(retry.row().await?.state, state::SUCCEEDED);
    assert_eq!(fixture.source().await?["info"]["state"], "ready");
    assert_eq!(
        fixture.source().await?["info"]["head"],
        json!(native.head()?)
    );
    assert_eq!(native.read(&directory, "binary")?, BINARY);
    assert!(native.branch_exists(&branch)?);
    assert_eq!(fixture.world.began(CommandKind::GitWorktree), 2);
    pending_input_only(&fixture).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prepare_abort_keeps_installed_checkout_bytes_and_retry_reconciles_its_original_repository()
-> TestResult {
    let (fixture, native, _) = fixture().await?;
    let mut plan = fixture.plan(false).await?;
    let Action::Prepare { source, .. } = &mut plan.action else {
        return Err("prepare plan absent".into());
    };
    source.repository_url = Some(URL.into());
    let (job, preparing, _) = fixture.begin(plan).await?;
    applied_checkpoint(&job, &preparing).await?;
    let answer = job.row().await?.plan["file_result"].clone();
    std::fs::write(
        native.files.root().root_path().join("web/code"),
        b"later person's bytes\0\xff",
    )?;
    fixture
        .engine
        .end_job(&job, Some(&preparing), failure())
        .await?;
    assert_eq!(job.row().await?.state, state::FAILED);
    assert_eq!(
        job.row().await?.plan["file_source_cleanup"]["removed"],
        false
    );
    assert_eq!(native.read("web", "code")?, b"later person's bytes\0\xff");
    let failed = fixture.source().await?;
    assert_eq!(failed["native_result"], answer);
    assert_eq!(failed["native_started"], true);
    let plan: Plan = serde_json::from_value(failed["native_plan"].clone())?;
    assert_eq!(
        json!(plan),
        json!(preparing.plan),
        "kept checkout retries the original native identity"
    );
    let (retry, _, _) = fixture.begin(plan).await?;
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&retry)).await??;
    assert_eq!(retry.row().await?.state, state::SUCCEEDED);
    let ready = fixture.source().await?;
    assert_eq!(ready["info"]["kind"], "repository");
    for member in ["repository_url", "branch", "head", "directory"] {
        assert_eq!(ready["info"][member], answer[member]);
    }
    assert_eq!(native.read("web", "code")?, b"later person's bytes\0\xff");
    assert_eq!(native.read("web", "binary")?, BINARY);
    pending_input_only(&fixture).await
}

async fn write_inline_recipe(fixture: &Fixture, preparing: &Preparing) -> TestResult {
    let (kind, directory, frozen, worktree) = match &preparing.plan.action {
        Action::Prepare {
            source, directory, ..
        } => (SourceKind::Repository, directory, source, None),
        Action::Worktree { source, body, .. } => (
            SourceKind::Worktree,
            &body.choice.directory,
            source,
            Some(&body.choice),
        ),
        _ => return Err("inline source action absent".into()),
    };
    let mut tx = fixture.world.pool.begin().await?;
    threads::lock(&mut tx, fixture.thread)
        .await?
        .ok_or("thread absent")?;
    let mut unit = Unit::new(&mut tx, fixture.clock.now());
    let seq = unit.stamp(fixture.thread).await?;
    let info = SourceInfo {
        kind,
        directory: directory.clone(),
        repository_url: frozen.repository_url.clone(),
        branch: frozen.branch.clone(),
        head: None,
        state: SourceState::Pending,
        job: None,
        reason: None,
    };
    source::set_source(&mut *unit.conn, fixture.thread, &json!({ "info": info, "source": frozen, "worktree": worktree,
        "prepare": true, "caller": preparing.plan.caller, "token": preparing.plan.token, "native_plan": preparing.plan }), seq).await?;
    unit.finish().await?;
    tx.commit().await?;
    Ok(())
}

async fn retained_start(fixture: &Fixture, parent: &Job, preparing: &Preparing) -> TestResult {
    parent
        .remember(&json!({ "mode": "start", "generation": preparing.plan.generation }))
        .await?;
    sqlx::query("UPDATE environments SET generation=$2 WHERE id=$1")
        .bind(preparing.plan.environment)
        .bind(preparing.plan.generation)
        .execute(fixture.world.pool.pool())
        .await?;
    Ok(())
}

async fn run_inline(parent: &Job, preparing: &Preparing) -> Result<Done, Stop> {
    provision_files(
        parent,
        preparing.plan.environment,
        preparing.plan.machine,
        preparing.plan.generation,
        preparing
            .provisioning_runtime
            .ok_or_else(|| Failure::new("internal", "the inline fixture runtime is missing"))?,
        "",
        false,
    )
    .await
}

async fn renew_after_clock_change(fixture: &Fixture, parent: &Job) -> TestResult {
    let row = parent.row().await?;
    let runner: Id = serde_json::from_value(row.plan["runner"]["id"].clone())?;
    let mut conn = fixture.world.pool.acquire().await?;
    assert!(
        rows::claim(
            &mut conn,
            parent.id(),
            runner,
            fixture.clock.now() + chrono::Duration::minutes(1),
            fixture.clock.now()
        )
        .await?
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inline_from_now_recovers_complete_and_expired_capture_with_its_full_recipe_not_a_prior_threads_tag()
-> TestResult {
    for expired in [false, true] {
        let (fixture, native, _) = fixture().await?;
        let (parent, mut preparing, _) = inline_parent(&fixture, false).await?;
        let effect = crate::ids::derived(
            "source.provision.files",
            &[parent.id().as_bytes(), fixture.thread.as_bytes()],
        );
        let snapshot = crate::ids::derived("worktree.current-files", &[effect.as_bytes()]);
        let frozen = RepositorySource {
            repository_url: None,
            branch: None,
            secret_name: None,
        };
        preparing.plan.action = Action::Worktree {
            body: GitWorktree {
                environment_id: fixture.world.environment,
                job_id: effect,
                snapshot_id: Some(snapshot),
                choice: WorktreeChoice {
                    repository_dir: "repo".into(),
                    branch: "work/inline-current".into(),
                    from: WorktreeFrom::Now {
                        state: orch_api_contract::source::WorktreeNow::Now,
                    },
                    directory: "web".into(),
                },
            },
            source: frozen,
            record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
            max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
        };
        write_inline_recipe(&fixture, &preparing).await?;
        let mut prior = preparing.plan.clone();
        prior.thread = Some(new_id());
        parent.remember(&json!({ "file_source_attempt": prior, "file_result": { "directory": "previous-thread", "head": "previous-head" } })).await?;
        std::fs::write(
            native.files.root().root_path().join("repo/code"),
            b"captured dirty source\0\xff",
        )?;
        let recorded = native.record(&fixture, snapshot)?;
        let hash = recorded.manifest_blob.ok_or("recorded manifest absent")?;
        let size = i64::try_from(
            std::fs::metadata(
                native
                    .states
                    .store()
                    .root()
                    .join("manifests")
                    .join(hash.to_string()),
            )?
            .len(),
        )?;
        {
            let mut tx = fixture.world.pool.begin().await?;
            storage::register_blob(&mut tx, &hash, size, fixture.clock.now()).await?;
            source::begin_recorded(
                &mut tx,
                &storage::NewSnapshot {
                    id: snapshot,
                    thread_id: Some(fixture.thread),
                    environment_id: fixture.world.environment,
                    env_generation: preparing.plan.generation,
                    label: "Synthetic current state",
                    selection: &json!({ "include": ["repo"], "exclude": [] }),
                    created_by: Some(fixture.world.ada),
                    at: fixture.clock.now(),
                },
                "repo",
                "turn",
            )
            .await?;
            assert!(
                source::finish_recorded(&mut tx, snapshot, &recorded, fixture.clock.now()).await?
            );
            tx.commit().await?;
        }
        let Action::Worktree { body, .. } = &preparing.plan.action else {
            return Err("inline worktree absent".into());
        };
        // The native receipt exists, but the parent still carries the preceding thread's tag.
        let expected = orch_envhost::git_worktrees::create(
            &native.tool,
            &native.files,
            &native.states,
            body,
            native.generation,
            Time::new(fixture.clock.now()),
        )
        .await?;
        assert_eq!(native.read("web", "code")?, b"captured dirty source\0\xff");
        assert_eq!(native.read("web", "binary")?, BINARY);
        std::fs::write(
            native.files.root().root_path().join("repo/code"),
            b"later source contents",
        )?;
        if expired {
            let store = native.states.store();
            let (captured, _) = store.load(snapshot, Time::new(fixture.clock.now()))?;
            let metadata = store.root().join("roots").join(format!("{snapshot}.json"));
            let original_metadata = std::fs::read(&metadata)?;
            let reference = store
                .root()
                .join("git/refs/snapshots")
                .join(snapshot.to_string());
            assert!(
                reference.try_exists()?,
                "the original complete capture owns its reference"
            );
            let held = native.states.read_recorded(
                &native.files,
                &Index::new(orch_link_contract::commands::IndexMode::Live, 0),
                &orch_link_contract::source::FsRecordRead {
                    environment_id: fixture.world.environment,
                    snapshot_id: snapshot,
                    path: None,
                    export: false,
                },
                new_id(),
                Time::new(fixture.clock.now()),
            )?;
            fixture.clock.set(
                recorded.expires_at.ok_or("recording expiry absent")?.get()
                    + chrono::Duration::seconds(1),
            );
            renew_after_clock_change(&fixture, &parent).await?;
            let mut conn = fixture.world.pool.acquire().await?;
            assert!(
                source::expire_recorded(&mut conn, snapshot, fixture.clock.now())
                    .await?
                    .is_some()
            );
            drop(conn);
            let expired_at = Time::new(fixture.clock.now());
            assert!(
                store.pinned(snapshot, expired_at)?,
                "an active recorded read remains held after expiry"
            );
            assert_eq!(
                native.states.prune(&[snapshot], expired_at)?,
                0,
                "collection must preserve the active read"
            );
            assert!(reference.try_exists()?);
            drop(held);
            assert!(
                !store.pinned(snapshot, expired_at)?,
                "all native reads released the expired capture"
            );
            store.require_idle()?;
            assert_eq!(
                native.states.prune(&[snapshot], expired_at)?,
                1,
                "the completed journal no longer retains its expired capture reference"
            );
            assert!(
                !reference.try_exists()?,
                "prune must release the actual native capture reference"
            );
            // Collection keeps immutable identity/expiry metadata so this id cannot capture later
            // live bytes. has_root observes that tombstone, not usable recorded contents.
            assert!(store.has_root(snapshot));
            assert_eq!(std::fs::read(&metadata)?, original_metadata);
            assert!(
                matches!(
                    store.load(snapshot, captured.created_at),
                    Err(orch_envhost::error::HostError::NotFound(_))
                ),
                "even at its original valid time the collected tree is gone; recovery cannot reread it"
            );
        }
        fixture
            .world
            .machine
            .handler
            .on(CommandKind::FsRecord, |_| {
                panic!("recovery must never capture newer source bytes")
            });
        assert_eq!(run_inline(&parent, &preparing).await, Ok(Done::Finished));
        let row = parent.row().await?;
        assert_eq!(row.plan["file_source_attempt"], json!(preparing.plan));
        assert_ne!(row.plan["file_source_attempt"], json!(prior));
        assert_eq!(row.plan["file_result"], json!(expected));
        let shown = fixture.source().await?;
        assert_eq!(shown["info"]["state"], "ready");
        assert_eq!(shown["info"]["kind"], "worktree");
        assert_eq!(shown["info"]["job"]["id"], json!(parent.id()));
        assert_eq!(shown["native_plan"], json!(preparing.plan));
        for member in ["directory", "branch", "head"] {
            assert_eq!(shown["info"][member], json!(expected)[member]);
        }
        assert_eq!(native.read("web", "code")?, b"captured dirty source\0\xff");
        assert_eq!(native.read("web", "binary")?, BINARY);
        assert_eq!(fixture.world.began(CommandKind::GitWorktree), 1);
        let mut conn = fixture.world.pool.acquire().await?;
        assert_eq!(
            storage::snapshot(&mut conn, snapshot)
                .await?
                .ok_or("snapshot absent")?
                .state,
            if expired { "expired" } else { "complete" }
        );
        drop(conn);
        pending_input_only(&fixture).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inline_first_existing_directory_needs_neither_write_gate_nor_its_unused_missing_secret()
-> TestResult {
    let (fixture, native, _) = fixture().await?;
    let (parent, mut preparing, _) = inline_parent(&fixture, false).await?;
    let Action::Prepare { source, .. } = &mut preparing.plan.action else {
        return Err("inline prepare absent".into());
    };
    source.repository_url = Some(URL.into());
    source.secret_name = Some("missing-synthetic-secret".into());
    write_inline_recipe(&fixture, &preparing).await?;
    retained_start(&fixture, &parent, &preparing).await?;
    std::fs::create_dir(native.files.root().root_path().join("web"))?;
    std::fs::write(native.files.root().root_path().join("web/binary"), BINARY)?;
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(fixture.world.environment)
        .execute(fixture.world.pool.pool())
        .await?;
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','false') WHERE id=$1")
        .bind(fixture.world.machine_id).execute(fixture.world.pool.pool()).await?;
    native.files.gates().set(Gates {
        remote_write: false,
        ..Gates::OPEN
    });
    // Reconstruct the read-only answer/remember checkpoint before publication too. Its
    // retained Directory result must not be mistaken for an already-issued native fetch.
    assert_eq!(preparing.effect(&parent).await, Ok(Done::Finished));
    assert_eq!(parent.row().await?.plan["file_result"]["kind"], "directory");
    assert_eq!(fixture.source().await?["info"]["state"], "pending");
    assert_eq!(run_inline(&parent, &preparing).await, Ok(Done::Finished));
    let shown = fixture.source().await?;
    assert_eq!(shown["info"]["state"], "ready");
    assert_eq!(shown["info"]["kind"], "directory");
    assert!(shown["info"]["repository_url"].is_null());
    assert!(shown["info"]["head"].is_null());
    assert!(parent.row().await?.plan["file_prepare_started"].is_null());
    assert_eq!(fixture.world.began(CommandKind::FsStat), 1);
    assert_eq!(fixture.world.began(CommandKind::GitPrepare), 0);
    assert_eq!(fixture.world.began(CommandKind::GitPrepareCleanup), 0);
    assert_eq!(native.read("web", "binary")?, BINARY);
    pending_input_only(&fixture).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inline_missing_directory_still_obeys_each_write_gate_and_requires_a_secret_only_for_fetch()
-> TestResult {
    for (environment_write, machine_write, expected) in [
        (false, false, "workspace.remote_write_disabled"),
        (true, false, "policy.remote_write_disabled"),
        (true, true, "workspace.source_failed"),
    ] {
        let (fixture, native, _) = fixture().await?;
        let (parent, mut preparing, _) = inline_parent(&fixture, false).await?;
        let Action::Prepare { source, .. } = &mut preparing.plan.action else {
            return Err("inline prepare absent".into());
        };
        source.repository_url = Some(URL.into());
        source.secret_name = Some("missing-synthetic-secret".into());
        write_inline_recipe(&fixture, &preparing).await?;
        retained_start(&fixture, &parent, &preparing).await?;
        sqlx::query("UPDATE environments SET remote_write=$2 WHERE id=$1")
            .bind(fixture.world.environment)
            .bind(environment_write)
            .execute(fixture.world.pool.pool())
            .await?;
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}',$2) WHERE id=$1")
            .bind(fixture.world.machine_id).bind(json!(machine_write)).execute(fixture.world.pool.pool()).await?;
        native.files.gates().set(Gates {
            remote_write: machine_write,
            ..Gates::OPEN
        });
        let Err(reason) = run_inline(&parent, &preparing).await else {
            return Err("missing-directory preparation ignored a refusal".into());
        };
        assert!(
            matches!(&reason, Stop::Failed(failure) if failure.code == expected),
            "{reason:?}"
        );
        fixture
            .engine
            .end_job(&parent, Some(&AtFilesCheckpoint), reason)
            .await?;
        assert_eq!(parent.row().await?.state, state::FAILED);
        assert_eq!(fixture.source().await?["info"]["state"], "failed");
        assert_eq!(fixture.world.began(CommandKind::FsStat), 1);
        assert_eq!(fixture.world.began(CommandKind::GitPrepare), 0);
        assert!(!native.files.root().root_path().join("web").exists());
        pending_input_only(&fixture).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inline_retained_native_success_preserves_repository_identity_with_a_marker_or_an_older_command_receipt()
-> TestResult {
    for marker in [false, true] {
        let (fixture, native, _) = fixture().await?;
        let (parent, mut preparing, _) = inline_parent(&fixture, false).await?;
        let Action::Prepare { source, .. } = &mut preparing.plan.action else {
            return Err("inline prepare absent".into());
        };
        source.repository_url = Some(URL.into());
        write_inline_recipe(&fixture, &preparing).await?;
        retained_start(&fixture, &parent, &preparing).await?;
        if marker {
            completed(preparing.mark_prepare_started(&parent).await)?;
        }
        let Action::Prepare {
            directory,
            source,
            destination,
            effect_id,
            credential,
        } = &preparing.plan.action
        else {
            return Err("prepare absent".into());
        };
        let effect = effect_id.ok_or("inline effect absent")?;
        let body = GitPrepare {
            environment_id: preparing.plan.environment,
            job_id: effect,
            directory: directory.clone(),
            repository_url: source.repository_url.clone().ok_or("repository absent")?,
            branch: source.branch.clone(),
            destination: destination.clone(),
            credential: credential.map(orch_link_contract::commands::Secret::Ref),
        };
        // Reconstruct a crash after the command's durable success but before remember/publish.
        // In the pre-marker case the command row, rather than an ID inequality, proves issuance.
        let answer = completed(
            parent
                .command(Outgoing::new(
                    crate::ids::derived(
                        "git.prepare",
                        &[effect.as_bytes(), parent.id().as_bytes()],
                    ),
                    preparing.plan.machine,
                    SerialKey::Environment(preparing.plan.environment),
                    &body,
                )?)
                .await,
        )?;
        let mut prior = preparing.plan.clone();
        prior.thread = Some(new_id());
        parent.remember(&json!({ "file_source_attempt": prior, "file_result": { "directory": "previous-thread", "head": "wrong-head" } })).await?;
        std::fs::write(
            native.files.root().root_path().join("web/code"),
            b"later retained contents\0\xff",
        )?;
        assert_eq!(run_inline(&parent, &preparing).await, Ok(Done::Finished));
        let row = parent.row().await?;
        assert_eq!(row.plan["file_prepare_started"], json!(preparing.plan));
        assert_eq!(row.plan["file_source_attempt"], json!(preparing.plan));
        assert_eq!(row.plan["file_result"], answer);
        let shown = fixture.source().await?;
        assert_eq!(shown["info"]["kind"], "repository");
        assert_eq!(shown["info"]["state"], "ready");
        for member in ["directory", "repository_url", "branch", "head"] {
            assert_eq!(shown["info"][member], answer[member]);
        }
        assert_eq!(
            native.read("web", "code")?,
            b"later retained contents\0\xff"
        );
        assert_eq!(native.read("web", "binary")?, BINARY);
        assert_eq!(
            fixture.world.began(CommandKind::GitPrepare),
            1,
            "the command receipt is replayed, never rerun"
        );
        pending_input_only(&fixture).await?;
    }
    Ok(())
}
