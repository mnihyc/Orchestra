//! People as a system admin keeps them: listed with the proofs strangers have locked, made with a link to set a password, suspended and let in again, made and unmade system admins, and given a link to set a password anew.
//!
//! A person's standing is changed by the functions of the access engine that the operator's
//! command line calls as well (`orch_authz::standing`), with the signed-in admin as the actor of the
//! audit line. A change that touches a system admin, or makes one, is made only after the admin
//! has said that they know what it means; the last system admin who can sign in is neither
//! suspended nor made an ordinary person, and a request that asks for several things at once
//! does all of them or none.

use super::admin_of;

use super::invalid;

use super::page_size;

use super::read_ahead;

use super::record;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::sign_in::accounts::email_fits;

use crate::sign_in::accounts::taken;

use crate::sign_in::reset_links;

use crate::sign_in::Limits;

use crate::sign_in::NOT_KEPT;

use crate::sign_in::origin;

use chrono::DateTime;

use chrono::Utc;

use http::header::CACHE_CONTROL;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::pages::StoredKind;

use orch_api_contract::admin::AdminUser;

use orch_api_contract::admin::AdminUserCreate;

use orch_api_contract::admin::AdminUserCreateBody;

use orch_api_contract::admin::AdminUserCreateReply;

use orch_api_contract::admin::AdminUserList;

use orch_api_contract::admin::AdminUserListQuery;

use orch_api_contract::admin::AdminUserLock;

use orch_api_contract::admin::AdminUserOrgInvitationGive;

use orch_api_contract::admin::AdminUserOrgInvitationTakeBack;

use orch_api_contract::admin::AdminUserResetLinkCreate;

use orch_api_contract::admin::AdminUserResetLinkCreateBody;

use orch_api_contract::admin::AdminUserResetLinkCreateReply;

use orch_api_contract::admin::AdminUserStatus;

use orch_api_contract::admin::AdminUserUpdate;

use orch_api_contract::admin::AdminUserUpdateBody;

use orch_api_contract::admin::AdminUserUpdateReceipt;

use orch_api_contract::admin::AdminUserUpdateResponse;

use orch_api_contract::auth::SignInProof;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Page;

use orch_authz::standing;

use orch_authz::standing::Acting;

use orch_authz::standing::Refused;

use orch_db::audit::Actor;

use orch_db::queries::identity;

use orch_db::queries::identity::Bound;

use orch_db::queries::identity::NewUser;

use orch_db::queries::identity::PeopleFilter;

use orch_db::queries::identity::UserBrief;

use orch_db::queries::identity::UserRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use serde_json::json;

use std::collections::HashMap;
/// The language a person starts with when the admin who makes them cannot know theirs.
const DEFAULT_LOCALE: &str = "en";

/// The fewest and the most characters of a username: what the schema allows.
const USERNAME_CHARS: std::ops::RangeInclusive<usize> = 3..=32;

/// The fewest and the most characters of a display name: what the schema allows.
const DISPLAY_NAME_CHARS: std::ops::RangeInclusive<usize> = 1..=64;

/// The most characters of an address: what mail allows of the whole of one.
const EMAIL_MAX_CHARS: usize = 254;

/// The proofs whose wrong tries are counted for a name, with the word the count is kept under.
const PROOFS: [(SignInProof, &str); 3] = [
    (SignInProof::Password, "password"),
    (SignInProof::EmailCode, "email_code"),
    (SignInProof::Totp, "totp"),
];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(AdminUserCreate, creation::user_binding())
        .creation(AdminUserResetLinkCreate, creation::link_binding())
        .on(AdminUserList, list)
        .on(AdminUserCreate, create)
        .on_with_operation(
            AdminUserUpdate,
            crate::key_operations::direct::declared,
            update,
        )
        .on_with_operation(
            AdminUserResetLinkCreate,
            crate::key_operations::direct::declared,
            reset_link,
        )
        .on_with_operation(
            AdminUserOrgInvitationGive,
            crate::key_operations::direct::declared,
            org_invitation_give,
        )
        .on_with_operation(
            AdminUserOrgInvitationTakeBack,
            crate::key_operations::direct::declared,
            org_invitation_take_back,
        );
}

fn status_word(status: AdminUserStatus) -> &'static str {
    match status {
        AdminUserStatus::Active => "active",
        AdminUserStatus::Suspended => "suspended",
    }
}

/// Until when the tries in a count keep its proof locked, if they do now: the count holds as many
/// as it may, and the oldest of them that must go before another is let in is `window` old at
/// that moment. The same arithmetic as the sign-in's own.
fn locked_until(
    times: &[DateTime<Utc>],
    now: DateTime<Utc>,
    bound: &Bound,
) -> Option<DateTime<Utc>> {
    let mut kept: Vec<DateTime<Utc>> = times
        .iter()
        .copied()
        .filter(|time| *time > now - bound.window)
        .collect();
    kept.sort_unstable();
    let limit = usize::try_from(bound.limit).unwrap_or(usize::MAX).max(1);
    if kept.len() < limit {
        return None;
    }
    kept.get(kept.len() - limit)
        .map(|oldest| *oldest + bound.window)
}

/// The proofs that strangers' wrong tries have locked at the names of `people` now, by person.
/// The count of strangers is the one without the mark of a browser the person got in from.
async fn locks_of(
    conn: &mut Conn,
    people: &[UserRow],
    now: DateTime<Utc>,
    bound: &Bound,
) -> ApiResult<HashMap<Id, Vec<AdminUserLock>>> {
    let buckets: Vec<String> = people
        .iter()
        .flat_map(|person| {
            PROOFS
                .iter()
                .map(move |(_, what)| format!("{what}|u:{}", person.id))
        })
        .collect();
    if buckets.is_empty() {
        return Ok(HashMap::new());
    }
    let mut found: HashMap<Id, Vec<AdminUserLock>> = HashMap::new();
    for (bucket, times) in identity::throttle_rows(conn, &buckets).await? {
        let Some((what, rest)) = bucket.split_once('|') else {
            continue;
        };
        let (Some(user), Some((way, _))) = (
            rest.strip_prefix("u:").and_then(|id| id.parse::<Id>().ok()),
            PROOFS.iter().find(|(_, word)| *word == what),
        ) else {
            continue;
        };
        if let Some(until) = locked_until(&times, now, bound) {
            found.entry(user).or_default().push(AdminUserLock {
                way: *way,
                until: Time::new(until),
            });
        }
    }
    for locks in found.values_mut() {
        locks.sort_by_key(|lock| PROOFS.iter().position(|(way, _)| *way == lock.way));
    }
    Ok(found)
}

/// A person as a system admin reads them.
fn shown(row: &UserRow, locked: Vec<AdminUserLock>) -> AdminUser {
    AdminUser {
        base: refusals::user_ref(&UserBrief {
            id: row.id,
            kind: row.kind.clone(),
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            avatar_color: row.avatar_color,
        }),
        email: row.email.clone(),
        email_verified: row.email_confirmed(),
        status: if row.suspended() {
            AdminUserStatus::Suspended
        } else {
            AdminUserStatus::Active
        },
        is_system_admin: row.is_system_admin,
        totp_enabled: row.totp_on(),
        email_code_second_step: row.email_code_on(),
        locked,
        org_invited_at: row.org_invited_at.map(Time::new),
        created_at: Time::new(row.created_at),
        last_seen_at: row.last_seen_at.map(Time::new),
    }
}

/// The person `id` as a system admin reads them, in the caller's transaction.
async fn read_one(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<AdminUser> {
    let row = person(conn, id).await?;
    let bound = Limits::of(&ctx.app.config.auth).login;
    let mut locks = locks_of(conn, std::slice::from_ref(&row), ctx.now, &bound).await?;
    Ok(shown(&row, locks.remove(&row.id).unwrap_or_default()))
}

/// The person `id`; a service identity or something that is not there is nobody here.
async fn person(conn: &mut Conn, id: Id) -> ApiResult<UserRow> {
    identity::user_by_id(conn, id)
        .await?
        .filter(|row| row.kind == "human")
        .ok_or_else(ApiError::not_found)
}

/// The people of the system, by username.
async fn list(ctx: Ctx, query: AdminUserListQuery, (): ()) -> ApiResult<Page<AdminUser>> {
    admin_of(&ctx)?;
    let size = page_size(query.limit);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut conn = ctx.app.db.acquire().await?;
    let mut rows = identity::people(
        &mut conn,
        PeopleFilter {
            q,
            status: query.status.map(status_word),
            system_admin: query.system_admin,
        },
        query.cursor.as_deref(),
        read_ahead(size),
    )
    .await?;
    let more = rows.len() > size;
    rows.truncate(size);
    let bound = Limits::of(&ctx.app.config.auth).login;
    let mut locks = locks_of(&mut conn, &rows, ctx.now, &bound).await?;
    Ok(Page {
        next_cursor: more
            .then(|| rows.last().map(|row| row.username.clone()))
            .flatten(),
        items: rows
            .iter()
            .map(|row| shown(row, locks.remove(&row.id).unwrap_or_default()))
            .collect(),
    })
}

/// Whether `name` is a username as the schema allows: lower-case letters, digits, hyphens and
/// underscores, beginning with a letter or a digit.
fn username_fits(name: &str) -> bool {
    let plain = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    USERNAME_CHARS.contains(&name.len())
        && name.bytes().next().is_some_and(plain)
        && name
            .bytes()
            .all(|byte| plain(byte) || byte == b'_' || byte == b'-')
}

/// What the system's policies say a new personal org starts as, unless the admin says otherwise:
/// closed to further members, and flat.
/// Makes a person with their own org and a link with which they set a password.
async fn create(
    mut ctx: Ctx,
    (): (),
    body: AdminUserCreateBody,
) -> ApiResult<Answer<orch_api_contract::admin::AdminUserCreateResponse>> {
    admin_of(&ctx)?;
    let username = body.username.trim();
    let display_name = body.display_name.trim();
    let email = body
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty());
    if !username_fits(username) {
        return Err(invalid(
            "username",
            "must be 3 to 32 characters: lower-case letters, digits, - and _, beginning with a letter or a digit",
        ));
    }
    if !DISPLAY_NAME_CHARS.contains(&display_name.chars().count()) {
        return Err(invalid("display_name", "must be 1 to 64 characters"));
    }
    if let Some(email) = email
        && (!email_fits(email) || email.chars().count() > EMAIL_MAX_CHARS)
    {
        return Err(invalid("email", "is not an address"));
    }
    let makes_admin = body.is_system_admin == Some(true);
    let (mut tx, guard) = super::action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let personal_org =
        crate::sign_in::accounts::personal_org_start(&mut tx, body.personal_org.as_ref()).await?;
    let id = orch_types::new_id();
    let created = identity::create_user(
        &mut tx,
        &NewUser {
            id,
            username,
            display_name,
            email,
            // The admin typed the address; nobody has shown yet that it is the person's.
            email_confirmed: false,
            password_hash: None,
            locale: DEFAULT_LOCALE,
            is_system_admin: makes_admin,
            created_by: Some(admin),
            personal_org,
        },
        ctx.now,
    )
    .await
    .map_err(taken)?;
    if let Some(org) = created.personal_org_id {
        orch_authz::pages::start(&mut tx, StoredKind::Node, org, id, Time::new(ctx.now))
            .await
            .map_err(ApiError::internal)?;
    }
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        AuditAction::UserRegistered,
        Some(id),
        &format!("{who} made {username}"),
        json!({
            "by_admin": true,
            "system_admin": makes_admin,
            "personal_org": personal_org.map(|org| json!({ "closed": org.closed, "flat": org.flat })),
        }),
    )
    .await?;
    if makes_admin {
        record(
            &ctx,
            &mut tx,
            admin,
            orch_db::audit::action!(AuditAction::UserAdminGranted),
            Some(id),
            &format!("{username} was made a system admin"),
            json!({}),
        )
        .await?;
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
    }
    let link = reset_links::make(
        &mut tx,
        &ctx.app.config,
        &created.user,
        false,
        Actor::User(admin),
        &origin(&ctx),
        ctx.now,
    )
    .await?;
    let reply = AdminUserCreateReply {
        user: shown(&created.user, Vec::new()),
        reset_url: Some(link.url),
        expires_at: Time::new(link.expires_at),
        already_created: false,
    };
    orch_db::idempotency::creation::observe_with(
        &mut tx,
        Some(admin),
        "admin_user",
        id,
        json!({"link":link.id,"expires":Time::new(link.expires_at)}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(
        Answer::from(orch_api_contract::admin::AdminUserCreateResponse::full(
            reply,
        ))
        .header(CACHE_CONTROL, NOT_KEPT),
    )
}

/// What a refused act of the access engine comes to as an answer.
fn refused(error: Refused) -> ApiError {
    match error {
        Refused::Db(error) => error.into(),
        Refused::Authz(error) => ApiError::internal(error),
        Refused::NoSuchUser(_) => ApiError::not_found(),
        Refused::LastAdmin { .. } => ApiError::new(
            ErrorCode::AdminLastAdmin,
            "this is the only system admin who can sign in: they stay one, and stay able to",
        ),
    }
}

/// Suspends a person or lets them in again, and makes them a system admin or an ordinary person.
async fn update(
    mut ctx: Ctx,
    (): (),
    body: AdminUserUpdateBody,
) -> ApiResult<AdminUserUpdateResponse> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = super::action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let before = person(&mut tx, id).await?;
    let touches = body.status.is_some() || body.is_system_admin.is_some();
    let makes_admin = body.is_system_admin == Some(true) && !before.is_system_admin;
    if touches
        && (before.is_system_admin || makes_admin)
        && body.acknowledge_admin_risk != Some(true)
    {
        return Err(ApiError::new(
            ErrorCode::AdminRiskNotAcknowledged,
            "a change that makes a system admin, or touches one, is made only once the caller says they know what it means (acknowledge_admin_risk)",
        ));
    }
    let origin = origin(&ctx);
    let acting = Acting {
        by: Actor::User(admin),
        origin: &origin,
        now: ctx.now,
    };
    // What can only add an active admin comes first, what can take one away last, and each act
    // looks at the state the one before it left: the person who is the last admin can neither be
    // suspended nor unmade in the same breath.
    if body.status == Some(AdminUserStatus::Active) {
        standing::resume(&mut tx, id, &acting)
            .await
            .map_err(refused)?;
    }
    if body.is_system_admin == Some(true) {
        standing::grant_admin(&mut tx, id, &acting)
            .await
            .map_err(refused)?;
    }
    if body.is_system_admin == Some(false) {
        standing::revoke_admin(&mut tx, id, &acting)
            .await
            .map_err(refused)?;
    }
    if body.status == Some(AdminUserStatus::Suspended) {
        standing::suspend(&mut tx, id, &acting)
            .await
            .map_err(refused)?;
    }
    let after = updated_user(&ctx, &mut tx, guard.principal(), id).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = AdminUserUpdateResponse::Receipt(AdminUserUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        account_id: id,
    });
    if guard.principal().caller().grant().is_some() {
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
    }
    Ok(match after {
        Some(user) => AdminUserUpdateResponse::full(user),
        None => receipt,
    })
}

/// The accepted change does not depend on regaining its effects. Only this operation's
/// current Read may materialize existing account content for a key.
async fn updated_user(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
) -> ApiResult<Option<AdminUser>> {
    // Browser mutations keep their established projection in the same authorized
    // transaction, including a person's own accepted demotion or suspension.
    if principal.caller().grant().is_none() {
        return read_one(ctx, conn, id).await.map(Some);
    }
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    // Reuse the exact typed declaration and neutral account subject for the response;
    // no separate path resolver or parallel credential/permission check is introduced.
    let plan =
        crate::key_operations::direct::declared(ctx, conn, principal.caller(), (), ()).await?;
    let projection = ctx.clone();
    match plan
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                if !reader.has_system_admin_standing() {
                    return Err(refusals::system_admin_only(
                        "current system administration is required to read account content",
                    ));
                }
                read_one(&projection, conn, id).await
            })
        })
        .await
    {
        // The standing writer may itself end this key's current authentication or
        // ordinary administration. Keep its accepted result without private content.
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

/// Makes a link with which a person sets a new password.
async fn reset_link(
    mut ctx: Ctx,
    (): (),
    body: AdminUserResetLinkCreateBody,
) -> ApiResult<Answer<AdminUserResetLinkCreateReply>> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = super::action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let who = person(&mut tx, id).await?;
    let made = reset_links::make(
        &mut tx,
        &ctx.app.config,
        &who,
        body.clear_second_step == Some(true),
        Actor::User(admin),
        &origin(&ctx),
        ctx.now,
    )
    .await?;
    orch_db::idempotency::creation::observe_with(
        &mut tx,
        Some(admin),
        "admin_reset_link",
        made.id,
        json!({"expires":Time::new(made.expires_at)}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Answer::from(AdminUserResetLinkCreateReply {
        url: Some(made.url),
        expires_at: Time::new(made.expires_at),
        already_created: false,
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// Tells the person's open pages that whether they may make a team org changed, so that the
/// control which makes one asks again.
async fn tell_org_creation(conn: &mut Conn, person: Id) -> ApiResult<()> {
    orch_db::sequences::notify_user_event(conn, &[person], &json!({ "what": "org_creation" }))
        .await?;
    Ok(())
}

/// Gives a person one invitation to make a team org, which making one uses up while
/// `orgs.created_by` is `invited`. It is given whatever the setting says now, so that an admin can
/// invite before changing it. A person who holds one unused keeps it as it was: nothing changes,
/// and nothing is recorded.
async fn org_invitation_give(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = super::action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let who = person(&mut tx, id).await?;
    if identity::give_org_invitation(&mut tx, id, admin, ctx.now)
        .await?
        .is_some()
    {
        let by = guard.user().user.display_name.clone();
        record(
            &ctx,
            &mut tx,
            admin,
            orch_db::audit::action!(AuditAction::OrgInvitationGiven),
            Some(id),
            &format!("{by} invited {} to make a team org", who.username),
            json!({ "username": who.username }),
        )
        .await?;
        tell_org_creation(&mut tx, id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Takes back the invitation to make a team org that a person has not used. One that was used, or
/// never given, is not there to take: nothing changes, and nothing is recorded.
async fn org_invitation_take_back(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = super::action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let who = person(&mut tx, id).await?;
    if let Some(taken) = identity::take_org_invitation(&mut tx, id).await? {
        let by = guard.user().user.display_name.clone();
        record(
            &ctx,
            &mut tx,
            admin,
            orch_db::audit::action!(AuditAction::OrgInvitationTakenBack),
            Some(id),
            &format!(
                "{by} took back the invitation of {} to make a team org",
                who.username
            ),
            json!({
                "username": who.username,
                "given_by": taken.given_by,
                "given_at": Time::new(taken.given_at),
            }),
        )
        .await?;
        tell_org_creation(&mut tx, id).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[path = "users/creation.rs"]
pub mod creation;

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use chrono::Duration;
    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::<Utc>::UNIX_EPOCH + Duration::minutes(minutes)
    }

    fn bound() -> Bound {
        Bound {
            limit: 3,
            window: Duration::minutes(180),
            gap: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old() {
        let times = [at(0), at(10), at(20)];
        assert_eq!(
            locked_until(&times, at(30), &bound()),
            Some(at(180)),
            "three tries in the window: the first must age out before another is let in"
        );
        assert_eq!(locked_until(&times[..2], at(30), &bound()), None);
        assert_eq!(
            locked_until(&times, at(181), &bound()),
            None,
            "the first has aged out, so two are left"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old() {
        self::a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_username_is_what_the_schema_allows() {
        for good in ["ada", "a1b", "0day", "ada_l-1", &"a".repeat(32)] {
            assert!(username_fits(good), "{good}");
        }
        for bad in ["ad", "Ada", "_ada", "-ada", "ada lovelace", &"a".repeat(33)] {
            assert!(!username_fits(bad), "{bad}");
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_username_is_what_the_schema_allows() {
        self::a_username_is_what_the_schema_allows()
    }
}
