//! A valid composing context supplies no source sharing permission.
use super::*;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn a_valid_composer_origin_does_not_replace_native_sharing_authority() -> Proof {
    let s = Scene::new().await;
    let doc = document(&s).await;
    let chat = s.tree.topic.base.chat_id;
    let origin = orch_access_contract::audit::LinkOrigin::Chat { chat_id: chat };
    let mut conn = s.rig.world.conn().await;
    // Establish the real native send premise without performing a grant or message.
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.eve.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await?;
    orch_engine::link_origin::check(&mut conn, &principal, Some(origin)).await?;
    drop(conn);
    let body = gift(s.cai.id(), json!({"kind":"chat","chat_id":chat}));
    let before = counts(&s).await;
    let refused = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.eve.token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(refused.status, http::StatusCode::FORBIDDEN);
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert_eq!(counts(&s).await, before);
    assert!(attribution(&s, doc).await.is_empty());
    let allowed = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.ada.token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(allowed.status, http::StatusCode::OK);
    let records = attribution(&s, doc).await;
    assert_eq!(
        records,
        vec![json!({"kind":"chat","chat_id":chat,"by":s.ada.id()})]
    );
    let after = counts(&s).await;
    assert!(after.0 > before.0);
    assert_eq!(after.2, before.2, "link evidence creates no input or wake");
    Ok(())
}
