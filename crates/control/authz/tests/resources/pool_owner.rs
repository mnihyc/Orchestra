//! Credential placement checks the actual owner's account without creating another requester.
use super::*;
use orch_access_contract::pools::PathMember;
use orch_authz::environments::{NewEnvironment, Place, Wants};
use orch_db::queries::pools::Leaf;

#[tokio::test]
async fn a_provider_owners_lent_pool_eligibility_keeps_both_consents_and_its_complete_bounds() {
    let w = World::new().await;
    let owner = w.member("provider_pool_owner").await;
    let lender = w.member("provider_host_lender").await;
    let manager = w.member("provider_pool_manager").await;
    let provider = w.provider(owner, w.root, "Owned credential").await;
    let host = w.thing("host", lender, w.root, "Lent host").await;
    let template = w.thing("template", owner, w.root, "Owned template").await;
    let pool = w.thing("pool", manager, w.root, "Shared hosts").await;
    w.thing_row("host", host, P::HostUse, false).await;
    let member = orch_types::new_id();
    let consent = orch_types::new_id();
    let mut conn = w.conn().await;
    sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(pool)
        .bind(json!({"kind":"host","enabled":true,"policy":{"type":"ordered"},"lend_members":true,"members":[],"config":{}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,0,$3,true)",
    )
    .bind(pool)
    .bind(member)
    .bind(json!({"type":"single","resource":{"kind":"host","id":host}}))
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO pool_lending(id,resource_kind,resource_id,giver_id,matcher,options) VALUES($1,'host',$2,$3,$4,'{}')")
        .bind(consent).bind(host).bind(lender).bind(json!({"kind":"pool","pool_id":pool})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let allowance = w
        .grant("pool", pool, P::PoolUse, Holder::User { id: owner })
        .await;
    let leaf = Leaf::Host { id: host };
    let hops = [PathMember {
        pool_id: pool,
        member_id: member,
        revision: 1,
    }];
    let selected = orch_authz::pools::Selection {
        leaf: &leaf,
        path: &hops,
    };
    let ask = NewEnvironment {
        home: w.root,
        place: Place::Host {
            host,
            template,
            wants: Wants {
                resources: Default::default(),
                capabilities: vec![],
                source: None,
                storage: None,
            },
        },
        mounts: vec![],
    };
    assert!(
        !orch_authz::resources::provider_owner_host_use(
            &mut *w.conn().await,
            provider,
            host,
            w.now
        )
        .await
        .unwrap()
    );
    assert!(
        orch_authz::environments::provider_owner_host_creation_in_pool(
            &mut *w.conn().await,
            provider,
            &ask,
            &Default::default(),
            selected,
            w.now,
        )
        .await
        .unwrap(),
        "the actual owner's bounded loan is eligible without direct Host Use"
    );
    sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
        .bind(allowance)
        .bind(json!({"host":{"max_environments":0}}))
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    assert!(
        !orch_authz::environments::provider_owner_host_creation_in_pool(
            &mut *w.conn().await,
            provider,
            &ask,
            &Default::default(),
            selected,
            w.now,
        )
        .await
        .unwrap(),
        "a complete owner pool allowance remains a conjunctive bound"
    );
    sqlx::query("UPDATE permission_entries SET options='{}' WHERE id=$1")
        .bind(allowance)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    sqlx::query("UPDATE pool_lending SET removed_at=$2 WHERE id=$1")
        .bind(consent)
        .bind(w.now.get())
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    assert!(
        !orch_authz::environments::provider_owner_host_creation_in_pool(
            &mut *w.conn().await,
            provider,
            &ask,
            &Default::default(),
            selected,
            w.now,
        )
        .await
        .unwrap(),
        "the current member-side consent is still required"
    );
}
