//! Declared Job reading through the existing native alternatives and prepared progress cut.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::nodes::projection::groups;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::Source;

use orch_authz::Caller;
const JOB: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Job);

const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

pub(super) struct Reader;

pub(super) struct Value {
    prepared: PreparedRead,
    choice: subject::Choice,
    names: Option<HashMap<Id, String>>,
}

fn declaration(provider: bool) -> &'static Declaration<Source> {
    if provider {
        &sources::JOB_GET_PROVIDER
    } else {
        &sources::JOB_GET_ORDINARY
    }
}

pub(super) async fn resolved(
    conn: &mut Conn,
    choice: subject::Choice,
) -> ApiResult<(Option<KeyResolvedTarget>, Option<KeyResolvedTarget>)> {
    let objects = choice
        .namespace
        .into_iter()
        .chain(choice.subject)
        .collect::<Vec<_>>();
    let targets = Sources::from_objects(objects)
        .identity_targets(conn)
        .await?;
    let source = |object: Option<orch_access_contract::decisions::ObjectRef>| -> ApiResult<_> {
        object
            .map(|object| {
                targets
                    .get(&object)
                    .cloned()
                    .ok_or_else(ApiError::not_found)
            })
            .transpose()
    };
    Ok((source(choice.namespace)?, source(choice.subject)?))
}

async fn root(
    conn: &mut Conn,
    row: &JobRow,
    choice: subject::Choice,
) -> ApiResult<native::Facts<Source>> {
    let (namespace, _) = resolved(conn, choice).await?;
    Ok(native::Facts::one(
        Source::Job,
        JOB,
        super::subject::identity(row, namespace.as_ref()),
    ))
}

struct Bounds<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
}

impl subject::Selection for Bounds<'_> {
    fn accepts<'a>(
        &'a self,
        conn: &'a mut Conn,
        row: &'a JobRow,
        choice: subject::Choice,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let declaration = declaration(choice.provider_management);
            let plan = OperationPlan::new(self.ctx.route, declaration.operation)?;
            let (namespace, target) = resolved(conn, choice).await?;
            let job = subject::identity(row, namespace.as_ref());
            if !native::permits(&plan, declaration, self.caller, &Source::Job, JOB, &job)? {
                return Ok(false);
            }
            if let (Some(object), Some(target)) = (choice.subject, target)
                && !native::permits(
                    &plan,
                    declaration,
                    self.caller,
                    &Source::JobSubjects,
                    KeyTargetKind::Object(object.kind),
                    &target,
                )?
            {
                return Ok(false);
            }
            Ok(true)
        })
    }
}

/// Prime only from an actual declared native candidate's real Job identity. Its ordinary
/// gate is still required, and every final subject stays unresolved until actually selected.
async fn principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &JobRow,
) -> ApiResult<Principal> {
    if !native::modern(caller) {
        let known = native::bind_known(ctx, &sources::JOB_GET_ORDINARY, &[])?;
        return native::preparation_principal(ctx, conn, caller, &known).await;
    }
    for candidate in subject::candidates(conn, row).await? {
        let declaration = declaration(candidate.choice.provider_management);
        let fact = match root(conn, row, candidate.choice).await {
            Ok(fact) => fact,
            Err(error) if error.code() == ErrorCode::NotFound => continue,
            Err(error) => return Err(error),
        };
        let plan = OperationPlan::new(ctx.route, declaration.operation)?;
        let target = &fact.groups[0].1[0];
        if native::permits(&plan, declaration, caller, &Source::Job, JOB, target)? {
            let known = native::bind_known(ctx, declaration, &[fact])?;
            return native::preparation_principal(ctx, conn, caller, &known).await;
        }
    }
    Err(subject::scope_refused())
}

pub(crate) async fn named_machines(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &JobRow,
    declaration: &'static Declaration<Source>,
    cut: bool,
) -> ApiResult<(Option<HashMap<Id, String>>, native::Facts<Source>)> {
    let mut named = HashMap::new();
    let mut allowed = Vec::new();
    if native::modern(caller) && !cut {
        let selected = machine_name_sources(conn, std::slice::from_ref(row)).await?;
        let ids = selected.keys().copied().collect::<Vec<_>>();
        let targets = targets::machine_identities(conn, &ids).await?;
        let plan = OperationPlan::new(ctx.route, declaration.operation)?;
        for (id, name) in selected {
            let Some(target) = targets.get(&id) else {
                continue;
            };
            if native::permits_usage(
                &plan,
                declaration,
                caller,
                &Source::JobMachineNames,
                FacetUsage::ResponseContent,
                MACHINE,
                target,
            )? {
                named.insert(id, name);
                allowed.push((MACHINE, target.clone()));
            }
        }
    }
    Ok((
        native::modern(caller).then_some(named),
        native::Facts::collection(Source::JobMachineNames, groups(allowed)),
    ))
}

impl native::Selected for Reader {
    type Source = Source;
    type Query = ();
    type Value = Value;
    type Reply = Job;
    const DECLARATION: &'static Declaration<Source> = &sources::JOB_GET_ORDINARY;

    fn declaration(_: &(), value: &Value) -> &'static Declaration<Source> {
        declaration(value.choice.provider_management)
    }

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = row_of(conn, ctx.id("id")?).await?;
            let principal = principal(ctx, conn, caller, &row).await?;
            // Scope never turns a native progress-only view or a native non-current refusal
            // into a broader body by choosing a different otherwise permitted alternative.
            let ordinary = subject::read(conn, &principal, &row, None).await?;
            if !native::modern(caller) {
                return Ok((
                    Value {
                        prepared: PreparedRead {
                            row,
                            cut: ordinary.cut,
                        },
                        choice: ordinary,
                        names: None,
                    },
                    Vec::new(),
                ));
            }
            let mut choice =
                subject::read(conn, &principal, &row, Some(&Bounds { ctx, caller })).await?;
            choice.cut |= ordinary.cut;
            let (namespace, subject) = resolved(conn, choice).await?;
            let root = native::Facts::one(
                Source::Job,
                JOB,
                super::subject::identity(&row, namespace.as_ref()),
            );
            let (names, name_facts) = named_machines(
                ctx,
                conn,
                caller,
                &row,
                declaration(choice.provider_management),
                choice.cut,
            )
            .await?;
            let subject_facts = native::Facts::collection(
                Source::JobSubjects,
                groups(
                    choice
                        .subject
                        .zip(subject)
                        .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
                ),
            );
            Ok((
                Value {
                    prepared: PreparedRead {
                        row,
                        cut: choice.cut,
                    },
                    choice,
                    names,
                },
                vec![root, subject_facts, name_facts],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the Job reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            value.prepared.require_current(conn, principal).await?;
            value.prepared.render(conn, value.names.as_ref()).await
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let names = match (&old.names, &fresh.names) {
                (Some(old_names), Some(current)) => {
                    old_names.keys().all(|id| current.contains_key(id))
                        && (old_names.is_empty()
                            || old.prepared.machine_source() == fresh.prepared.machine_source())
                }
                (None, _) => true,
                _ => false,
            };
            Ok(old.prepared.preserves(&fresh.prepared)
                && old.choice.subject == fresh.choice.subject
                && old.choice.namespace == fresh.choice.namespace
                && old.choice.provider_management == fresh.choice.provider_management
                && names)
        })
    }
}
