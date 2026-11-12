//! Guest consent follows the actual organization home of a group chat.

use super::*;

#[tokio::test]
async fn group_chat_invitations_require_terms_from_the_group_home() {
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
    // Home placement is independently tested; redemption uses the real invitation path.
    sqlx::query("UPDATE chats SET home_id=$2 WHERE id=$1")
        .bind(group)
        .bind(department)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let placement: (Option<Id>, Option<Id>, Id) =
        sqlx::query_as("SELECT node_id,home_id,org_id FROM chats WHERE id=$1")
            .bind(group)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(placement, (None, Some(department), room.root));
    let guest = person(&room.world, "outside-research").await;
    let invitation = invite(&room.world, &room.ada, &offer(InvitationKind::Chat, group)).await;
    let shown = terms(&room.world, &invitation.token).await.unwrap();
    assert_eq!(shown.text, "Root terms\n\nDepartment terms");
    assert_eq!(
        shown.hash,
        hex::encode(Sha256::digest(shown.text.as_bytes()))
    );
    let root_only = hex::encode(Sha256::digest(b"Root terms"));
    assert_ne!(root_only, shown.hash);
    let refused = accept(&room.world, &guest, &invitation.token, Some(&root_only)).await;
    assert_eq!(refused.status, 409);
    let problem = refused.problem();
    assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
    assert_eq!(problem.extra["terms"], json!(shown));
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(group)
    .bind(guest.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        members, 0,
        "a root-only acknowledgement supplies no group membership"
    );
    let unchanged: (i32, i64) = sqlx::query_as(
        "SELECT uses,(SELECT count(*) FROM audit_log WHERE action='member.terms_accepted' AND actor_id=$2)
         FROM invitations WHERE id=$1",
    )
    .bind(invitation.invitation.id)
    .bind(guest.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, (0, 0));
    accept(&room.world, &guest, &invitation.token, Some(&shown.hash))
        .await
        .ok();
    let recorded: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT terms_hash,terms_accepted_at FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(group)
    .bind(guest.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(recorded, (Some(shown.hash.clone()), Some(room.world.now())));
    audit_acceptance(
        &room.world,
        invitation.invitation.id,
        department,
        guest.id(),
        &guest.person.username,
        &shown.hash,
    )
    .await;
}
