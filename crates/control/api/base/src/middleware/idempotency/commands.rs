//! Native creation commands are recorded on their actual queue transaction, before send or wait.

use super::Capture;

use super::Created;

use super::canonical;

use super::fingerprint;

use super::request_key;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_db::Conn;

use orch_db::idempotency::creation;

use orch_db::queries::link;

use orch_db::queries::link::CommandRow;

use orch_link_contract::commands::CommandKind;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde::Deserialize;

use serde::Serialize;
/// A declaration selects the native creating command, never an HTTP path or a guessed resource.
#[derive(Clone, Copy)]
pub struct CreationCommand {
    pub kind: CommandKind,
    pub identity: fn(&CommandRow) -> ApiResult<Option<Created>>,
}

pub struct CommandCapture {
    account: Id,
    key: String,
    native_kind: String,
    declaration: CreationCommand,
}

tokio::task_local! {static CURRENT: CommandCapture;}

pub(crate) fn captures(kind: CommandKind) -> bool {
    CURRENT
        .try_with(|capture| capture.declaration.kind == kind)
        .unwrap_or(false)
}

pub(crate) fn capture(
    capture: Option<&Capture>,
    declaration: Option<CreationCommand>,
) -> Option<CommandCapture> {
    let capture = capture?;
    Some(CommandCapture {
        account: capture.account,
        key: capture.key.clone(),
        native_kind: capture.record.native_kind.clone(),
        declaration: declaration?,
    })
}

pub(crate) async fn scope<T>(
    capture: Option<CommandCapture>,
    work: impl std::future::Future<Output = T>,
) -> T {
    match capture {
        Some(capture) => CURRENT.scope(capture, work).await,
        None => work.await,
    }
}

/// A factual reference only. It contains no body, result bytes, credential or forward authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    id: Id,
    machine: Id,
    kind: String,
    deadline: Time,
    fingerprint: Hash,
}

fn frame(row: &CommandRow) -> Json {
    canonical(serde_json::json!({
        "id": row.id,
        "machine": row.machine_id,
        "kind": row.kind,
        "serial": row.serial_key(),
        "body": row.body,
        "deadline": row.deadline_at,
    }))
}

fn seal(account: Id, key: &str, row: &CommandRow) -> ApiResult<Hash> {
    fingerprint(account, key, "orch.creation.native-command.v1", frame(row))
}

/// The issuer calls this after its native admission and insert, on that same transaction.
/// A cancelled HTTP waiter therefore cannot make an accepted operation look unperformed.
pub(crate) async fn observe(conn: &mut Conn, row: &CommandRow) -> ApiResult<()> {
    let Some(capture) = CURRENT
        .try_with(|capture| {
            (capture.declaration.kind.as_str() == row.kind).then(|| {
                (
                    capture.account,
                    capture.key.clone(),
                    capture.native_kind.clone(),
                    capture.declaration,
                )
            })
        })
        .ok()
        .flatten()
    else {
        return Ok(());
    };
    let (account, key, native_kind, declaration) = capture;
    let Some(mut created) = (declaration.identity)(row)? else {
        // Preparation commands can share a kind with the actual creator. Only the domain's
        // typed creating purpose records an accepted new identity at this queue boundary.
        return Ok(());
    };
    if created.kind != native_kind {
        return Err(ApiError::internal(
            "the native command differs from its creation declaration",
        ));
    }
    if created.data.is_null() {
        created.data = serde_json::json!({});
    }
    let fields = created
        .data
        .as_object_mut()
        .ok_or_else(|| ApiError::internal("the native command creation facts must be an object"))?;
    if fields.contains_key("native_command") {
        return Err(ApiError::internal(
            "the native command reference is supplied only by its issuer",
        ));
    }
    fields.insert(
        "native_command".into(),
        serde_json::to_value(Reference {
            id: row.id,
            machine: row.machine_id,
            kind: row.kind.clone(),
            deadline: Time::new(row.deadline_at),
            fingerprint: seal(account, &key, row)?,
        })
        .map_err(ApiError::internal)?,
    );
    creation::observe_with(conn, Some(account), &native_kind, created.id, created.data).await?;
    Ok(())
}

/// Read the actual accepted/pending/completed row as data under this account's retry key.
/// It does not queue, send, retime, reconcile, adopt or restore the first credential.
pub async fn load(ctx: &Ctx, conn: &mut Conn, created: &Created) -> ApiResult<Option<CommandRow>> {
    let key = request_key(ctx)?
        .ok_or_else(|| ApiError::internal("a native command replay has no key"))?;
    load_for(ctx.user()?.id(), &key, conn, created).await
}

async fn load_for(
    account: Id,
    key: &str,
    conn: &mut Conn,
    created: &Created,
) -> ApiResult<Option<CommandRow>> {
    let reference: Reference =
        serde_json::from_value(
            created.data.get("native_command").cloned().ok_or_else(|| {
                ApiError::internal("the creation has no native command reference")
            })?,
        )
        .map_err(ApiError::internal)?;
    let Some(row) = link::command(conn, reference.id).await? else {
        return Ok(None);
    };
    if row.machine_id != reference.machine
        || row.kind != reference.kind
        || Time::new(row.deadline_at) != reference.deadline
        || seal(account, key, &row)? != reference.fingerprint
    {
        return Err(ApiError::internal(
            "the native command no longer matches its creation reference",
        ));
    }
    Ok(Some(row))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_db::idempotency;

    use orch_db::idempotency::Replay;

    use orch_db::idempotency::creation::Record;

    use orch_db::queries::link::NewCommand;

    use orch_testkit::clock;

    use orch_testkit::database::TestDb;

    use orch_types::new_id;

    use serde_json::json;
    pub type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn identity(row: &CommandRow) -> ApiResult<Option<Created>> {
        Ok(Some(Created {
            kind: "native_test".into(),
            id: row.id,
            data: Json::Null,
        }))
    }

    async fn claim(db: &TestDb, account: Id, key: &str) -> Result<Capture, orch_db::DbError> {
        let record = Record::pending(Hash::of(b"opaque creation body"), "native_test", new_id());
        let mut conn = db.pool.acquire().await?;
        idempotency::keep(
            &mut conn,
            account,
            key,
            &Replay {
                method: "POST".into(),
                path: "/native-creations".into(),
                status: 201,
                response: serde_json::to_value(&record).unwrap(),
            },
            clock::start(),
        )
        .await?;
        Ok(Capture {
            account,
            key: key.into(),
            record,
        })
    }

    async fn outcome(db: &TestDb, account: Id, key: &str) -> Result<Record, orch_db::DbError> {
        let value: Json =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(account)
                .bind(key)
                .fetch_one(&db.pool)
                .await?;
        Record::read(value)
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn each_native_creating_command_is_atomic_with_its_exact_data_only_reference() -> Proof {
        crate::routes::resources::sealing::install_master_key(orch_db::secrets::MasterKey::new(
            [7; 32],
        ));
        let db = TestDb::fresh().await;
        let account = new_id();
        let mut conn = db.pool.acquire().await?;
        sqlx::query("INSERT INTO users(id,username,display_name,avatar_color) VALUES($1,'native-command-maker','Maker',0)")
            .bind(account).execute(&mut *conn).await?;
        let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
            &mut conn,
            account,
            "native-command-receipts",
        )
        .await?;
        drop(conn);
        for kind in [
            CommandKind::McpImport,
            CommandKind::FsCopy,
            CommandKind::BrowserProfile,
        ] {
            for commit in [false, true] {
                let key = new_id().to_string();
                let capture = claim(&db, account, &key).await?;
                let command_capture =
                    super::capture(Some(&capture), Some(CreationCommand { kind, identity }));
                let id = new_id();
                let at = clock::start();
                let body = json!({"environment_id":new_id(),"name":"private supplied profile","from":"private source","to":"private target"});
                creation::scope(
                    Some(capture),
                    super::scope(command_capture, async {
                        let mut tx = db.pool.begin().await?;
                        link::insert_command_with_authority(
                            &mut tx,
                            &NewCommand {
                                id,
                                machine_id: machine,
                                kind: kind.as_str(),
                                body: &body,
                                serial_key: Some("environment"),
                                deadline_at: at + chrono::Duration::minutes(1),
                            },
                            at,
                            None,
                        )
                        .await?;
                        let row = link::command(&mut tx, id)
                            .await?
                            .ok_or("accepted command absent")?;
                        super::observe(&mut tx, &row).await?;
                        // A later domain filing adds its own identity facts without losing the queue.
                        creation::observe_with(
                            &mut tx,
                            Some(account),
                            "native_test",
                            id,
                            json!({"environment_id":body["environment_id"]}),
                        )
                        .await?;
                        if commit {
                            tx.commit().await?;
                        } else {
                            tx.rollback().await?;
                        }
                        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
                    }),
                )
                .await?;
                let saved = outcome(&db, account, &key).await?;
                let mut conn = db.pool.acquire().await?;
                if !commit {
                    assert!(saved.outcome.is_none());
                    assert!(link::command(&mut conn, id).await?.is_none());
                    continue;
                }
                let created = saved.outcome.clone().ok_or("committed reference absent")?;
                assert_eq!(created.id, id);
                let reference: Reference =
                    serde_json::from_value(created.data["native_command"].clone())?;
                assert_eq!(reference.kind, kind.as_str());
                let encoded = serde_json::to_string(&created)?;
                assert!(!encoded.contains("private supplied profile"));
                assert!(!encoded.contains("private source"));
                assert!(!encoded.contains("private target"));
                let row = load_for(account, &key, &mut conn, &created)
                    .await?
                    .ok_or("stored row absent")?;
                assert_eq!(row.body, body);
                assert_eq!(row.state, "queued");
                assert!(load_for(new_id(), &key, &mut conn, &created).await.is_err());
                assert!(
                    load_for(account, &new_id().to_string(), &mut conn, &created)
                        .await
                        .is_err()
                );
                let changed = json!({"name":"substituted"});
                sqlx::query("UPDATE commands SET body=$2 WHERE id=$1")
                    .bind(id)
                    .bind(&changed)
                    .execute(&mut *conn)
                    .await?;
                assert!(load_for(account, &key, &mut conn, &created).await.is_err());
                sqlx::query("UPDATE commands SET body=$2,state='succeeded',result=$3 WHERE id=$1")
                    .bind(id)
                    .bind(&body)
                    .bind(json!({"actual":"current native result"}))
                    .execute(&mut *conn)
                    .await?;
                assert_eq!(
                    load_for(account, &key, &mut conn, &created)
                        .await?
                        .ok_or("stored result absent")?
                        .result,
                    Some(json!({"actual":"current native result"}))
                );
                assert_eq!(outcome(&db, account, &key).await?, saved);
            }
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_each_native_creating_command_is_atomic_with_its_exact_data_only_reference()
    -> ::futures::future::LocalBoxFuture<
        'static,
        crate::middleware::idempotency::commands::tests::Proof,
    > {
        ::std::boxed::Box::pin(
            self::each_native_creating_command_is_atomic_with_its_exact_data_only_reference(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation() -> Proof {
        crate::routes::resources::sealing::install_master_key(orch_db::secrets::MasterKey::new(
            [7; 32],
        ));
        let db = TestDb::fresh().await;
        let account = new_id();
        let key = new_id().to_string();
        let mut conn = db.pool.acquire().await?;
        sqlx::query("INSERT INTO users(id,username,display_name,avatar_color) VALUES($1,'native-preparation-owner','Owner',0)").bind(account).execute(&mut *conn).await?;
        let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
            &mut conn,
            account,
            "native-preparation",
        )
        .await?;
        drop(conn);
        let capture = claim(&db, account, &key).await?;
        let model = CreationCommand {
            kind: CommandKind::McpImport,
            identity: |row| {
                if row.body["phase"] == "read" {
                    Ok(None)
                } else {
                    identity(row)
                }
            },
        };
        let read = new_id();
        let created = new_id();
        let scope = super::capture(Some(&capture), Some(model));
        creation::scope(
            Some(capture),
            super::scope(scope, async {
                for (id, phase) in [(read, "read"), (created, "create")] {
                    let mut tx = db.pool.begin().await?;
                    let at = clock::start();
                    let body = json!({"phase":phase});
                    link::insert_command(
                        &mut tx,
                        &NewCommand {
                            id,
                            machine_id: machine,
                            kind: "mcp.import",
                            body: &body,
                            serial_key: Some("machine"),
                            deadline_at: at + chrono::Duration::minutes(1),
                        },
                        at,
                    )
                    .await?;
                    let row = link::command(&mut tx, id)
                        .await?
                        .ok_or("actual native phase absent")?;
                    super::observe(&mut tx, &row).await?;
                    tx.commit().await?;
                    let record = outcome(&db, account, &key).await?;
                    if phase == "read" {
                        assert!(record.outcome.is_none());
                    } else {
                        assert_eq!(record.outcome.ok_or("creating phase absent")?.id, created);
                    }
                }
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            }),
        )
        .await?;
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation()
    -> ::futures::future::LocalBoxFuture<
        'static,
        crate::middleware::idempotency::commands::tests::Proof,
    > {
        ::std::boxed::Box::pin(
            self::a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation(),
        )
    }
}
