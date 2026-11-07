//! Ordinary hidden-reference names are independent of whether a key has an operation hook.
use super::*;
use orch_api_contract::approvals::ApprovalGet;

#[tokio::test]
async fn a_known_private_document_request_keeps_the_authored_label_without_disclosing_its_name() {
    let s = Office::new().await;
    assert!(
        !can_document(
            &s.desk.world,
            s.requester.id(),
            s.document,
            Permission::DocView
        )
        .await
    );
    assert!(
        can_document(
            &s.desk.world,
            s.desk.ada.id(),
            s.document,
            Permission::DocView
        )
        .await
    );
    let body = s.document();
    let label = body.items[0].label.as_ref().unwrap();
    assert_ne!(
        label, DOCUMENT,
        "the authored label is independent of the private name"
    );
    let made = s.native(&body).await;
    let approval = id(&made["approvals"][0]["id"]);
    let batch = checked(
        get(&s.desk.world, &s.requester.token, id(&made["batch_id"])).await,
        StatusCode::OK,
    );
    let requester = checked(
        s.desk
            .world
            .app
            .call(ApprovalGet, &[&approval])
            .token(&s.requester.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let giver = checked(
        s.desk
            .world
            .app
            .call(ApprovalGet, &[&approval])
            .token(&s.desk.ada.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(made["approvals"][0]["request"]["label"], json!(label));
    assert!(
        giver.to_string().contains(DOCUMENT),
        "the ordinary entitled giver must retain the real name"
    );
    // All three actual native projections and the giver positive have completed before this oracle.
    for (surface, card) in [
        ("create", &made["approvals"][0]),
        ("batch", &batch["approvals"][0]),
        ("approval", &requester),
    ] {
        assert!(
            !card.to_string().contains(DOCUMENT),
            "{surface} leaked the actual private name: title={},object={},node={},request={}",
            card["title"],
            card["object"],
            card["node"],
            card["request"]
        );
    }
}
