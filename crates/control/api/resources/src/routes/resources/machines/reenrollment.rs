//! Same native replacement token, retained original issuer and optional installation-command Read.

use super::Installer;

use super::enrollment_commands;

use super::require_machine;

use crate::certificates::NewToken;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::machine_sources as sources;

use orch_api_contract::machine_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::EnrollmentToken;

use orch_api_contract::resources::MachineEnrollmentAgainReceipt;

use orch_api_contract::resources::MachineEnrollmentAgainResponse;

use orch_api_contract::resources::MachineEnrollmentAgainTokenCreate;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::resources;

use orch_db::queries::resources::MachineRow;

use orch_db::queries::resources::reenrollment;

use orch_engine::enrollment_authority::AgainOrigin;

use orch_types::Id;
pub(super) struct Reservations;

#[derive(Clone)]
pub(super) struct Subject {
    row: MachineRow,
    host: Option<Id>,
}

async fn subject(conn: &mut Conn, id: Id) -> ApiResult<Subject> {
    let row = resources::machine(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let host = reenrollment::host_of(conn, id).await?;
    Ok(Subject { row, host })
}

async fn require(conn: &mut Conn, principal: &Principal, value: &Subject) -> ApiResult<()> {
    if let Some(host) = value.host {
        authorization::require_resource(
            conn,
            principal,
            ResourceKind::Host,
            host,
            Permission::HostManage,
            Policy::default(),
        )
        .await
    } else {
        require_machine(conn, principal, value.row.id, Permission::AccessOwn).await
    }
}

fn preserved(old: &Subject, fresh: &Subject) -> bool {
    old.row.id == fresh.row.id
        && old.row.owner_id == fresh.row.owner_id
        && old.row.host_id == fresh.row.host_id
        && old.row.runtime_id == fresh.row.runtime_id
        && old.host == fresh.host
        && fresh.row.lost_at.is_none()
        && old.row.roles == fresh.row.roles
        && old.row.facts.get("installation") == fresh.row.facts.get("installation")
}

impl native::Stored for Reservations {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::AGAIN];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = subject(conn, ctx.id("id")?).await?;
            let target = targets::machine_subject(conn, value.row.id).await?;
            Ok((
                value,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, value))
    }
}

struct Issued {
    machine: Id,
    made: NewToken,
}

struct Projection;

struct Refusal;

impl super::acknowledgement::Body for Refusal {
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &sources::AGAIN;
    const WITH_MACHINE: bool = false;
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let value = subject(conn, row.id).await?;
            require(conn, principal, &value).await
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _row: &'a MachineRow,
        _machine: Option<super::Machine>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

impl native::reply::Projection for Projection {
    type Source = Source;
    type Input = Issued;
    type Subject = Subject;
    type Value = ();
    type Reply = EnrollmentToken;
    const READ_SOURCE: Source = Source::Governor;
    fn declaration(_input: &Issued) -> &'static Declaration<Source> {
        &sources::AGAIN
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Issued,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = subject(conn, input.machine).await?;
            let target = targets::machine_subject(conn, value.row.id).await?;
            Ok((
                value,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Issued,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if value.row.lost_at.is_some() {
                return Err(ApiError::not_found());
            }
            require(conn, principal, value).await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Issued,
        _subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Issued,
        prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<EnrollmentToken>> {
        Box::pin(async move { response(ctx, &prepared.subject, &input.made) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Issued,
        old: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(preserved(&old.subject, &fresh.subject)) })
    }
}

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_optional_operation(
        MachineEnrollmentAgainTokenCreate,
        native::stored_deferred::<Reservations, _, _>,
        create,
    );
}

async fn create(ctx: Ctx, (): (), (): ()) -> ApiResult<MachineEnrollmentAgainResponse> {
    let projection = ctx.clone();
    match create_on(ctx).await {
        Ok(value) => Ok(value),
        Err(error) => Err(super::acknowledgement::refusal::<Refusal>(&projection, error).await?),
    }
}

async fn create_on(ctx: Ctx) -> ApiResult<MachineEnrollmentAgainResponse> {
    let machine = ctx.id("id")?;
    let (credential, until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let value = subject(&mut tx, machine).await?;
    if value.row.lost_at.is_some() {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::PlanBlocked,
            "A machine declared lost needs a new enrollment and identity",
        ));
    }
    require(&mut tx, initial.principal(), &value).await?;
    if !resources::lock_resource(&mut tx, "machine", machine).await? {
        return Err(ApiError::not_found());
    }
    sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
        .bind(machine)
        .fetch_all(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let value = subject(&mut tx, machine).await?;
    require(&mut tx, guard.principal(), &value).await?;
    if value.row.lost_at.is_some() {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::PlanBlocked,
            "A machine declared lost needs a new enrollment and identity",
        ));
    }
    let installation = super::installation_config(&ctx).await?;
    if value.host.is_some() {
        super::require_node_installable(&installation)?;
    } else {
        super::require_installable(&installation)?;
    }
    let service = ctx.app.link.certificates();
    let prepared: crate::certificates::PreparedAgainToken = service
        .prepare_again_token(&mut tx, ctx.user()?.id(), machine)
        .await
        .map_err(|fault| ApiError::internal(format!("{}: {}", fault.code, fault.detail)))?;
    let issuer = AgainOrigin::capture(
        &mut tx,
        guard.principal().caller(),
        credential,
        until,
        prepared.binding(),
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(ApiError::engine)?;
    let made = service
        .apply_again_token(&mut tx, prepared, &issuer, || guard.check_deadline(&ctx))
        .await?
        .map_err(|fault| ApiError::internal(format!("{}: {}", fault.code, fault.detail)))?;
    let (accepted, until) = issuer
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original enrollment issuer has ended",
            )
        })?;
    guard.check_deadline(&ctx)?;
    super::acknowledgement::deadline(&ctx, &accepted, until)?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = MachineEnrollmentAgainResponse::Receipt(MachineEnrollmentAgainReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: machine,
        token: made.token.clone(),
        expires_at: made.expires_at,
        receipt: Some(made.receipt.clone()),
    });
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(
        match native::reply::render::<Projection>(&ctx, &accepted, Issued { machine, made }).await?
        {
            Some(value) => MachineEnrollmentAgainResponse::Full(Box::new(value)),
            None => receipt,
        },
    )
}

fn response(ctx: &Ctx, subject: &Subject, made: &NewToken) -> ApiResult<EnrollmentToken> {
    let row = &subject.row;
    let installer = if subject.host.is_some() {
        Installer::Node
    } else {
        Installer::Connector
    };
    let installation = super::installation_config(ctx).await?;
    let mut commands = enrollment_commands(&installation, &made.token, installer);
    if matches!(installer, Installer::Connector) {
        let roles = row
            .roles
            .iter()
            .filter(|role| role.as_str() != "node")
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        commands
            .linux
            .push_str(&format!(" --roles {}", super::sh_word(&roles)));
        commands
            .macos
            .push_str(&format!(" --roles {}", super::sh_word(&roles)));
        commands
            .windows
            .push_str(&format!(" -Roles {}", super::ps_word(&roles)));
    }
    // Retained and restored installations must address the same directory, including named instances.
    if let Some(directory) = row
        .facts
        .pointer("/installation/data_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let posix = format!(" --data-dir {}", super::sh_word(directory));
        commands.linux.push_str(&posix);
        if matches!(installer, Installer::Connector) {
            commands.macos.push_str(&posix);
            commands
                .windows
                .push_str(&format!(" -DataDir {}", super::ps_word(directory)));
        }
    }
    if matches!(installer, Installer::Node)
        && let Some(path) = row
            .facts
            .pointer("/installation/config_file")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    {
        commands
            .linux
            .push_str(&format!(" --config {}", super::sh_word(path)));
    }
    if row.facts.pointer("/installation/service") == Some(&serde_json::Value::Null)
        && let (Some(executable), Some(directory)) = (
            row.facts
                .pointer("/installation/executable")
                .and_then(serde_json::Value::as_str),
            row.facts
                .pointer("/installation/data_dir")
                .and_then(serde_json::Value::as_str),
        )
    {
        let mut start = format!(
            "{} --data-dir {}",
            super::sh_word(executable),
            super::sh_word(directory)
        );
        if matches!(installer, Installer::Node)
            && let Some(path) = row
                .facts
                .pointer("/installation/config_file")
                .and_then(serde_json::Value::as_str)
        {
            start.push_str(&format!(" --config {}", super::sh_word(path)));
        }
        commands
            .linux
            .push_str(&format!(" --no-service && {start} run"));
        if matches!(installer, Installer::Connector) {
            commands
                .macos
                .push_str(&format!(" --no-service && {start} run"));
            commands.windows.push_str(&format!(
                " -NoService; if ($?) {{ & {} --data-dir {} run }}",
                super::ps_word(executable),
                super::ps_word(directory)
            ));
        }
    }
    Ok(EnrollmentToken {
        receipt: Some(made.receipt.clone()),
        commands,
        token: made.token.clone(),
        expires_at: made.expires_at,
    })
}
