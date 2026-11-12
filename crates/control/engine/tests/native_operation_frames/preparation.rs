//! Real stored credentials and native known-source facts never complete an unknown binding.

use super::*;
use orch_access_contract::decisions::ObjectRef;
use orch_api_contract::nodes::ThreadAdopt;
use orch_engine::operation_scope::continuing::preparation as phase;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Topic,
    Provider,
}
struct AdoptionSources;
impl Source for AdoptionSources {
    type Role = Role;
    type Input = (Id, Id);
    type Value = (Id, Id);
    const ROUTE: Route = ThreadAdopt::ROUTE;
    // This native prerequisite reads its Provider before selecting a binding. Actual
    // harness setup needs its further declared native write roles, supplied by its owner.
    const DECLARATION: &'static Declaration<Role> = &Declaration {
        operation: "ThreadAdopt",
        bindings: &[
            Binding {
                source: Role::Topic,
                target: TargetResolver::TypedPath {
                    parameter: "id",
                    kind: KeyTargetKind::Object(ObjectKind::Topic),
                },
                usage: FacetUsage::Required,
                cardinality: Cardinality::One,
            },
            Binding {
                source: Role::Provider,
                target: TargetResolver::Resolved("native_session"),
                usage: FacetUsage::Required,
                cardinality: Cardinality::One,
            },
        ],
    };
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Self::Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let topic = targets::node(conn, input.0)
                .await
                .map_err(EngineError::unreadable)?;
            if topic.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
                return Err(Error::Refused);
            }
            let provider = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.1,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                *input,
                vec![
                    Facts::one(
                        Role::Topic,
                        KeyTargetKind::Object(ObjectKind::Thread),
                        topic,
                    ),
                    Facts::one(
                        Role::Provider,
                        KeyTargetKind::Object(ObjectKind::Provider),
                        provider,
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !nodes::check(
                conn,
                principal,
                value.0,
                Permission::TopicStartThreads,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: value.1,
                },
                Permission::ProviderView,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(value.0)
                .fetch_one(conn)
                .await
                .map_err(orch_db::DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

async fn original(s: &Fixture) -> Key {
    let id = new_id();
    let policy: KeyScope = serde_json::from_value(json!({"rules":[
        {"target":{"kind":"topic","id":s.world.topic,"reach":"self"},"resources":["thread"],
            "allow":[{"operation":"ThreadAdopt","actions":["write"]}]},
        {"target":{"kind":"provider","id":s.world.provider,"reach":"self"},"resources":["provider"],
            "allow":[{"operation":"ThreadAdopt","actions":["read"]}]}
    ]}))
    .unwrap();
    assert!(policy.validate().is_empty());
    let expires = s.clock.now() + chrono::Duration::hours(1);
    let mut tx = s.world.pool.begin().await.unwrap();
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id,
            user_id: s.world.ada,
            name: "Native preparation origin",
            token_hash: Hash::of(id.as_bytes()).as_bytes(),
            scopes: &[],
            node_id: None,
            at: s.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    let caller = Caller::token(
        s.world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))
            .unwrap()
            .for_token(id),
    );
    let (_, facts) = AdoptionSources::prepare(
        &mut tx,
        &caller,
        &(s.world.topic, s.world.provider),
        s.clock.time(),
    )
    .await
    .unwrap();
    let known =
        sources::bind_declared(AdoptionSources::ROUTE, AdoptionSources::DECLARATION, &facts)
            .unwrap();
    assert!(
        known.required_facets().is_err(),
        "a selected Environment/Agent binding does not exist yet"
    );
    let caller = known.preparation_caller(&caller).unwrap();
    let principal = Principal::load(&mut tx, caller, s.clock.time())
        .await
        .unwrap();
    let input = (s.world.topic, s.world.provider);
    AdoptionSources::require(&mut tx, &principal, &input, &input)
        .await
        .unwrap();
    let origin = CapturedOperation::capture(principal.caller()).unwrap();
    tx.commit().await.unwrap();
    Key { id, origin }
}

#[tokio::test]
async fn a_native_known_source_phase_never_admits_its_unknown_later_business_binding() {
    let s = Fixture::new().await;
    let key = original(&s).await;
    let input = (s.world.topic, s.world.provider);
    let mut tx = s.world.pool.begin().await.unwrap();
    assert!(
        continuing::prepare::<AdoptionSources>(&mut tx, &s.clock, &key.origin, input)
            .await
            .is_err(),
        "the full effect frame still needs every canonical required role"
    );
    tx.rollback().await.unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let frame = phase::prepare::<AdoptionSources>(&mut tx, &s.clock, &key.origin, input)
        .await
        .unwrap();
    assert_eq!(*frame.value(), input);
    assert_eq!(
        frame.principal().caller().admitted_operation(),
        Some("ThreadAdopt")
    );
    let parts = frame.into_parts();
    parts.first_effect.check(&s.clock).unwrap();
    let opened = parts.opened;
    tx.commit().await.unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    let _fresh = opened.refresh(&mut tx, &s.clock).await.unwrap();
    assert!(
        continuing::prepare::<AdoptionSources>(&mut tx, &s.clock, &key.origin, input)
            .await
            .is_err(),
        "refresh never promotes a preparation witness to full business admission"
    );
}

#[tokio::test]
async fn a_native_preparation_restores_its_original_key_before_further_work() {
    let s = Fixture::new().await;
    let key = original(&s).await;
    let mut tx = s.world.pool.begin().await.unwrap();
    let parts = phase::prepare::<AdoptionSources>(
        &mut tx,
        &s.clock,
        &key.origin,
        (s.world.topic, s.world.provider),
    )
    .await
    .unwrap()
    .into_parts();
    let opened = parts.opened;
    tx.commit().await.unwrap();
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(s.clock.now())
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let mut tx = s.world.pool.begin().await.unwrap();
    assert!(opened.refresh(&mut tx, &s.clock).await.is_err());
}

#[tokio::test]
async fn a_native_preparation_checks_the_real_key_after_its_actual_source_lock_wait() {
    for expired in [false, true] {
        let s = Fixture::new().await;
        let key = original(&s).await;
        let mut blocker = s.world.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(s.world.topic)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let db = s.world.pool.clone();
        let clock = s.clock.clone();
        let input = (s.world.topic, s.world.provider);
        let request = tokio::spawn(async move {
            let mut tx = db.begin().await?;
            let frame =
                phase::prepare::<AdoptionSources>(&mut tx, &clock, &key.origin, input).await?;
            frame.into_parts().first_effect.check(&clock)
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!request.is_finished());
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'SELECT id FROM nodes%FOR UPDATE%')")
                    .bind(pid).fetch_one(s.world.pool.pool()).await.unwrap();
                if waiting {break;}
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        if expired {
            s.clock.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.is_ok(), !expired);
    }
}
