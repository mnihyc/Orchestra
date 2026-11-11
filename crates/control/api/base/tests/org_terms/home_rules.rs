//! A group invitation resolves consent from its actual native home.
use super::*;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn group_chat_invitations_require_terms_from_the_group_home() -> Proof {
    let room = chat_world::Room::new().await;
    let department = make_department(&room.world, &room.ada, room.root, "Research", "research")
        .await
        .base
        .id;
    room.world
        .set_at(Target::Node(room.root), "org.terms", json!("Root terms"))
        .await;
    room.world
        .set_at(
            Target::Node(department),
            "org.terms",
            json!("Department terms"),
        )
        .await;
    let group = room
        .group(&room.ada, &[&room.bea], Some("Research group"))
        .await
        .base
        .id;
    // The actual stored home is fixture setup; redemption uses its real HTTP handler.
    sqlx::query("UPDATE chats SET home_id=$2 WHERE id=$1")
        .bind(group)
        .bind(department)
        .execute(&room.world.db.pool)
        .await?;
    let native = orch_db::queries::chats::chat(&mut *room.world.conn().await, group)
        .await?
        .ok_or("the group is absent")?;
    assert_eq!(native.home(), department);
    let guest = person(&room.world, "outside-research").await;
    let invitation = invite(&room.world, &room.ada, &offer(InvitationKind::Chat, group)).await;
    let shown = terms(&room.world, &invitation.token)
        .await
        .ok_or("the actual terms are absent")?;
    assert_eq!(shown.text, "Root terms\n\nDepartment terms");
    let root_only = hex::encode(Sha256::digest(b"Root terms"));
    let refused = accept(&room.world, &guest, &invitation.token, Some(&root_only)).await;
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(
        refused.problem().code,
        ErrorCode::InvitationTermsNotAccepted
    );
    untouched(&room.world, invitation.invitation.id, room.root, guest.id()).await;
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(group)
    .bind(guest.id())
    .fetch_one(&room.world.db.pool)
    .await?;
    assert_eq!(members, 0);
    accept(&room.world, &guest, &invitation.token, Some(&shown.hash))
        .await
        .ok();
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(group)
    .bind(guest.id())
    .fetch_one(&room.world.db.pool)
    .await?;
    assert_eq!(members, 1);
    audit_acceptance(
        &room.world,
        invitation.invitation.id,
        department,
        guest.id(),
        &guest.person.username,
        &shown.hash,
    )
    .await;
    Ok(())
}
