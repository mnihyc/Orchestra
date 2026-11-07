//! Native creation parents, independent response Read, and actual wait/publication boundaries.

use super::*;
use orch_api_contract::directory::{
    GroupDelete, OrgGroupCreate, OrgGroupCreateBody, OrgGroupList, OrgGroupListQuery,
    ServiceAccountDelete,
};
use orch_api_core::testing::TestApp;

#[derive(Clone, Copy, Debug)]
enum Creation {
    Group,
    Service,
}
impl Creation {
    const ALL: [Self; 2] = [Self::Group, Self::Service];
    fn operation(self) -> &'static str {
        match self {
            Self::Group => "OrgGroupCreate",
            Self::Service => "OrgServiceAccountCreate",
        }
    }
    fn business(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Service => "account",
        }
    }
    fn wrong_business(self) -> &'static str {
        match self {
            Self::Group => "account",
            Self::Service => "group",
        }
    }
    fn receipt(self, id: Id) -> Json {
        match self {
            Self::Group => json!({"projection":"receipt","group_id":id}),
            Self::Service => json!({"projection":"receipt","service_account_id":id}),
        }
    }
    fn rules(self, org: Id, actions: &[&str], names: bool) -> Vec<Json> {
        let mut rules = vec![rule(
            self.operation(),
            "org",
            org,
            "subtree",
            self.business(),
            actions,
        )];
        if names {
            rules.push(rule(
                self.operation(),
                "org",
                org,
                "subtree",
                "org",
                &["read"],
            ));
        }
        rules
    }
    async fn call(self, app: TestApp, secret: String, org: Id, name: String) -> RawReply {
        match self {
            Self::Group => {
                let call = app.call(OrgGroupCreate, &[&org]);
                call.token(&secret)
                    .body(&OrgGroupCreateBody {
                        name,
                        description: Some("freshly supplied description".into()),
                    })
                    .send_bytes()
                    .await
            }
            Self::Service => {
                let call = app.call(OrgServiceAccountCreate, &[&org]);
                call.token(&secret)
                    .body(&OrgServiceAccountCreateBody {
                        username: name,
                        display_name: "Fresh service display".into(),
                    })
                    .send_bytes()
                    .await
            }
        }
    }
    async fn stored(self, s: &Scene, name: &str) -> Option<(Id, Id, String)> {
        let sql = match self {
            Self::Group => "SELECT id,org_id,xmin::text FROM groups WHERE name=$1",
            Self::Service => "SELECT id,service_org_id,xmin::text FROM users WHERE username=$1",
        };
        sqlx::query_as(sql)
            .bind(name)
            .fetch_optional(&s.world.db.pool)
            .await
            .unwrap()
    }
    async fn ordinary(self, s: &Scene, id: Id) -> Json {
        let rows = match self {
            Self::Group => checked(
                s.world
                    .app
                    .call(OrgGroupList, &[&s.org])
                    .token(&s.boss.token)
                    .query(&OrgGroupListQuery::default())
                    .send_bytes()
                    .await,
                StatusCode::OK,
            ),
            Self::Service => checked(
                s.world
                    .app
                    .call(OrgServiceAccountList, &[&s.org])
                    .token(&s.boss.token)
                    .send_bytes()
                    .await,
                StatusCode::OK,
            ),
        };
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| match self {
                Self::Group => row["id"] == json!(id),
                Self::Service => row["user"]["id"] == json!(id),
            })
            .unwrap()
            .clone()
    }
    async fn delete(self, s: &Scene, id: Id) {
        let reply = match self {
            Self::Group => {
                s.world
                    .app
                    .call(GroupDelete, &[&id])
                    .token(&s.boss.token)
                    .send_bytes()
                    .await
            }
            Self::Service => {
                s.world
                    .app
                    .call(ServiceAccountDelete, &[&id])
                    .token(&s.boss.token)
                    .send_bytes()
                    .await
            }
        };
        assert_eq!(
            reply.status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
    }
}

async fn wait_for_blocker(
    s: &Scene,
    request: &tokio::task::JoinHandle<RawReply>,
    pid: i32,
    query: &str,
) {
    // Match the existing native PostgreSQL-wait watchdog, not an elapsed-time premise.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the native wait must be reached");
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
            )
            .bind(pid)
            .bind(query)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the request reaches the actual native SQL wait");
}

#[tokio::test]
async fn creation_binds_the_actual_org_with_the_created_business_kind_and_keeps_manage_only_receipts()
 {
    for kind in Creation::ALL {
        let s = Scene::new().await;
        let writer = key(&s.world, s.boss.id(), kind.rules(s.org, &["manage"], false)).await;
        let response = checked(
            kind.call(
                s.world.app.clone(),
                writer.secret,
                s.org,
                "created-without-read".into(),
            )
            .await,
            StatusCode::OK,
        );
        let stored = kind.stored(&s, "created-without-read").await.unwrap();
        assert_eq!(
            stored.1, s.org,
            "the true creation parent is the actual Org"
        );
        assert_eq!(response, kind.receipt(stored.0));

        for (name, rules) in [
            (
                "wrong-business",
                vec![rule(
                    kind.operation(),
                    "org",
                    s.org,
                    "subtree",
                    kind.wrong_business(),
                    &["manage"],
                )],
            ),
            ("read-alone", kind.rules(s.org, &["read"], false)),
            (
                "wrong-parent",
                kind.rules(s.boss.person.personal_org, &["manage"], false),
            ),
        ] {
            let restricted = key(&s.world, s.boss.id(), rules).await;
            let response = kind
                .call(s.world.app.clone(), restricted.secret, s.org, name.into())
                .await;
            assert_eq!(response.status, StatusCode::FORBIDDEN, "{kind:?}: {name}");
            assert!(
                kind.stored(&s, name).await.is_none(),
                "a refused scope changes nothing"
            );
        }
    }
}

#[tokio::test]
async fn creation_readback_uses_its_own_read_and_preserves_native_full_bodies_and_optional_names() {
    for kind in Creation::ALL {
        let s = Scene::new().await;
        let full = key(
            &s.world,
            s.boss.id(),
            kind.rules(s.org, &["manage", "read"], true),
        )
        .await;
        let body = checked(
            kind.call(
                s.world.app.clone(),
                full.secret,
                s.org,
                "complete-native".into(),
            )
            .await,
            StatusCode::OK,
        );
        let id = kind.stored(&s, "complete-native").await.unwrap().0;
        assert_eq!(
            body,
            kind.ordinary(&s, id).await,
            "the complete native projection is unchanged"
        );

        let minimal = key(
            &s.world,
            s.boss.id(),
            kind.rules(s.org, &["manage", "read"], false),
        )
        .await;
        let body = checked(
            kind.call(
                s.world.app.clone(),
                minimal.secret,
                s.org,
                "minimal-native".into(),
            )
            .await,
            StatusCode::OK,
        );
        let id = kind.stored(&s, "minimal-native").await.unwrap().0;
        let mut native = kind.ordinary(&s, id).await;
        if matches!(kind, Creation::Group) {
            native.as_object_mut().unwrap().remove("place");
        }
        assert_eq!(
            body, native,
            "optional home names cannot become a hidden creation prerequisite"
        );

        let mut rules = kind.rules(s.org, &["manage"], false);
        let (other, action) = match kind {
            Creation::Group => ("OrgGroupList", "read"),
            Creation::Service => ("OrgServiceAccountList", "manage"),
        };
        rules.push(rule(
            other,
            "org",
            s.org,
            "subtree",
            kind.business(),
            &[action],
        ));
        let unrelated = key(&s.world, s.boss.id(), rules).await;
        let body = checked(
            kind.call(
                s.world.app.clone(),
                unrelated.secret,
                s.org,
                "unrelated-read".into(),
            )
            .await,
            StatusCode::OK,
        );
        let id = kind.stored(&s, "unrelated-read").await.unwrap().0;
        assert_eq!(
            body,
            kind.receipt(id),
            "another operation cannot lend response Read"
        );
    }
}

#[tokio::test]
async fn creation_scope_never_grants_the_native_members_manage_permission() {
    for kind in Creation::ALL {
        let s = Scene::new().await;
        let viewer = key(
            &s.world,
            s.member.id(),
            kind.rules(s.org, &["manage", "read"], true),
        )
        .await;
        let response = kind
            .call(
                s.world.app.clone(),
                viewer.secret,
                s.org,
                "no-native-management".into(),
            )
            .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&response.raw).contains("Private directory root"));
        assert!(kind.stored(&s, "no-native-management").await.is_none());
    }
}

#[tokio::test]
async fn creation_rechecks_its_real_key_after_the_actual_keeper_row_wait() {
    for kind in Creation::ALL {
        for expire in [false, true] {
            let s = Scene::new().await;
            let writer = key(&s.world, s.boss.id(), kind.rules(s.org, &["manage"], false)).await;
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(s.org)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = tokio::spawn(kind.call(
                s.world.app.clone(),
                writer.secret,
                s.org,
                "keeper-wait".into(),
            ));
            wait_for_blocker(&s, &request, pid, "SELECT id FROM nodes%FOR UPDATE%").await;
            assert!(kind.stored(&s, "keeper-wait").await.is_none());
            if expire {
                s.world.advance(Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let response = tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap();
            if expire {
                assert!(matches!(
                    response.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ));
                assert!(kind.stored(&s, "keeper-wait").await.is_none());
            } else {
                let response = checked(response, StatusCode::OK);
                let id = kind.stored(&s, "keeper-wait").await.unwrap().0;
                assert_eq!(response, kind.receipt(id));
            }
        }
    }
}

#[tokio::test]
async fn completed_creation_keeps_its_receipt_after_activity_wait_expiry_without_replaying() {
    for kind in Creation::ALL {
        for expire in [false, true] {
            let s = Scene::new().await;
            let writer = key(
                &s.world,
                s.boss.id(),
                kind.rules(s.org, &["manage", "read"], true),
            )
            .await;
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = tokio::spawn(kind.call(
                s.world.app.clone(),
                writer.secret,
                s.org,
                "publication-wait".into(),
            ));
            wait_for_blocker(&s, &request, pid, "%INSERT INTO api_token_activity%").await;
            let before = kind.stored(&s, "publication-wait").await.unwrap();
            assert_eq!(
                before.1, s.org,
                "the native effect committed before bookkeeping"
            );
            if expire {
                s.world.advance(Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let response = checked(
                tokio::time::timeout(std::time::Duration::from_secs(30), request)
                    .await
                    .unwrap()
                    .unwrap(),
                StatusCode::OK,
            );
            if expire {
                assert_eq!(response, kind.receipt(before.0));
            } else {
                assert_eq!(response, kind.ordinary(&s, before.0).await);
            }
            assert_eq!(
                kind.stored(&s, "publication-wait").await.unwrap(),
                before,
                "optional publication cannot replay or rewrite the completed creation"
            );
        }
    }
}

#[tokio::test]
async fn completed_creation_rechecks_current_read_without_readmitting_completed_manage() {
    for kind in Creation::ALL {
        for keep_read in [false, true] {
            let s = Scene::new().await;
            let writer = key(
                &s.world,
                s.boss.id(),
                kind.rules(s.org, &["manage", "read"], false),
            )
            .await;
            let mut blocker = s.world.db.pool.begin().await.unwrap();
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let request = tokio::spawn(kind.call(
                s.world.app.clone(),
                writer.secret,
                s.org,
                "current-response-read".into(),
            ));
            wait_for_blocker(&s, &request, pid, "%INSERT INTO api_token_activity%").await;
            let before = kind.stored(&s, "current-response-read").await.unwrap();
            let actions = if keep_read {
                &["read"][..]
            } else {
                &["manage"][..]
            };
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":kind.rules(s.org, actions, false)})).unwrap();
            assert!(scope.validate().is_empty());
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(json!(scope))
                    .execute(&s.world.db.pool),
            )
            .await
            .expect("the current key rule changes while its answer waits for activity")
            .unwrap();
            blocker.commit().await.unwrap();
            let response = checked(
                tokio::time::timeout(std::time::Duration::from_secs(30), request)
                    .await
                    .unwrap()
                    .unwrap(),
                StatusCode::OK,
            );
            if keep_read {
                let mut native = kind.ordinary(&s, before.0).await;
                if matches!(kind, Creation::Group) {
                    native.as_object_mut().unwrap().remove("place");
                }
                assert_eq!(
                    response, native,
                    "completed Manage is not re-admitted for Read"
                );
            } else {
                assert_eq!(
                    response,
                    kind.receipt(before.0),
                    "current Manage cannot lend the lost response Read"
                );
            }
            assert_eq!(
                kind.stored(&s, "current-response-read").await.unwrap(),
                before,
                "response authority changes cannot replay the writer"
            );
        }
    }
}

#[tokio::test]
async fn creation_publication_cannot_replace_the_original_subject_with_another_at_the_same_org() {
    for kind in Creation::ALL {
        let s = Scene::new().await;
        let writer = key(
            &s.world,
            s.boss.id(),
            kind.rules(s.org, &["manage", "read"], true),
        )
        .await;
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let request = tokio::spawn(kind.call(
            s.world.app.clone(),
            writer.secret,
            s.org,
            "original-subject".into(),
        ));
        wait_for_blocker(&s, &request, pid, "%INSERT INTO api_token_activity%").await;
        let original = kind.stored(&s, "original-subject").await.unwrap().0;
        kind.delete(&s, original).await;
        let replacement_name = if matches!(kind, Creation::Group) {
            "original-subject"
        } else {
            "replacement-subject"
        };
        checked(
            kind.call(
                s.world.app.clone(),
                s.boss.token.clone(),
                s.org,
                replacement_name.into(),
            )
            .await,
            StatusCode::OK,
        );
        let replacement = kind.stored(&s, replacement_name).await.unwrap().0;
        assert_ne!(original, replacement);
        assert_eq!(
            kind.ordinary(&s, replacement).await[match kind {
                Creation::Group => "org_id",
                Creation::Service => "token_count",
            }],
            match kind {
                Creation::Group => json!(s.org),
                Creation::Service => json!(0),
            }
        );
        blocker.commit().await.unwrap();
        let response = checked(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
            StatusCode::OK,
        );
        assert_eq!(
            response,
            kind.receipt(original),
            "a replacement at the same home is not the old source"
        );
    }
}
