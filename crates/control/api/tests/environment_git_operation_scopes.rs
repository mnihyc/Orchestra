//! Scoped Git publication uses the existing environment file door and queued job.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::source::{
    EnvironmentGitCommit, EnvironmentGitCommitBody, EnvironmentGitPush, EnvironmentGitPushBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{identity, jobs};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use workspace::Site;

#[derive(Clone, Copy, Debug)]
enum Publication {
    Commit,
    Push,
}

impl Publication {
    fn operation(self) -> &'static str {
        match self {
            Self::Commit => "EnvironmentGitCommit",
            Self::Push => "EnvironmentGitPush",
        }
    }

    fn actions(self) -> Json {
        match self {
            Self::Commit => json!(["manage"]),
            // Push transmits existing repository content to the reviewed remote.
            Self::Push => json!(["manage", "read"]),
        }
    }

    async fn request(self, site: &Site, token: &str) -> RawReply {
        let query = ThroughThreadQuery {
            thread: Some(site.thread),
        };
        match self {
            Self::Commit => {
                site.app()
                    .call(EnvironmentGitCommit, &[&site.environment])
                    .token(token)
                    .query(&query)
                    .body(&EnvironmentGitCommitBody {
                        path: "web".into(),
                        paths: vec!["src/private-change.rs".into()],
                        message: "Private supplied commit message".into(),
                        expected_head: None,
                        expected_status: Hash::of(b"reviewed status"),
                    })
                    .send_bytes()
                    .await
            }
            Self::Push => {
                site.app()
                    .call(EnvironmentGitPush, &[&site.environment])
                    .token(token)
                    .query(&query)
                    .body(&EnvironmentGitPushBody {
                        path: "web".into(),
                        remote: "origin".into(),
                        expected_remote_url: "https://example.test/private/repository.git".into(),
                        branch: "work/reviewed".into(),
                        expected_head: "0123456789012345678901234567890123456789".into(),
                        set_upstream: Some(true),
                    })
                    .send_bytes()
                    .await
            }
        }
    }
}

struct Key {
    id: Id,
    secret: String,
}

async fn key(site: &Site, user: Id, target: Json, allow: Json) -> Key {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{"target":target,"allow":allow}]})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = json!(scope);
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *site.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Git publication boundary control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: site.world.now(),
            expires_at: Some(site.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    Key { id, secret }
}

fn target(kind: &str, id: Id) -> Json {
    json!({"kind":kind,"id":id,"reach":"self"})
}

async fn accepted(publication: Publication) {
    let site = Site::live().await;
    let issued = key(
        &site,
        site.ada.id(),
        target("environment", site.environment),
        json!([{"operation":publication.operation(),"actions":publication.actions()}]),
    )
    .await;
    let reply = publication.request(&site, &issued.secret).await;
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let shown: Json = serde_json::from_slice(&reply.raw).unwrap();
    let id = serde_json::from_value(shown["id"].clone()).unwrap();
    let row = jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.environment_id, Some(site.environment));
    assert_eq!(row.node_id, Some(site.thread));
    assert_eq!(row.created_by, Some(site.ada.id()));
    assert_eq!(row.plan["token"], json!(issued.id));
    let origin: orch_authz::CapturedOperation =
        serde_json::from_value(row.plan["publication_origin"].clone()).unwrap();
    assert_eq!(origin.operation(), publication.operation());
    assert_eq!(origin.user(), site.ada.id());
    assert_eq!(shown["state"], "queued");
    for field in [
        "result",
        "log",
        "error",
        "call_recovery",
        "started_at",
        "finished_at",
    ] {
        assert!(shown[field].is_null(), "the queued receipt has no {field}");
    }
    assert_eq!(shown["progress"]["percent"], 0);
    assert!(shown["progress"]["message"].is_null());
    assert!(shown["progress"]["activity"]["waiting_for"].is_null());
    assert_eq!(shown["progress"]["activity"]["stopping"], false);
    let person = identity::user_by_id(&mut *site.world.conn().await, site.ada.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown["created_by"]["id"], json!(person.id));
    assert_eq!(
        shown["created_by"]["display_name"],
        json!(person.display_name)
    );
    assert!(
        row.result.is_none()
            && row.error.is_none()
            && row.started_at.is_none()
            && row.finished_at.is_none()
    );
    assert!(row.plan.get("active_command").is_none());
    assert!(row.progress.get("log").is_none());
    for private in [
        "private-change.rs",
        "Private supplied commit message",
        "private/repository.git",
        "0123456789012345678901234567890123456789",
    ] {
        assert!(
            !String::from_utf8_lossy(&reply.raw).contains(private),
            "the publication receipt contains no source or repository content"
        );
    }
}

#[tokio::test]
async fn commit_accepts_exact_manage_on_the_environment_and_retains_its_origin() {
    accepted(Publication::Commit).await;
}

#[tokio::test]
async fn push_accepts_its_own_read_and_manage_on_the_environment_and_retains_its_origin() {
    accepted(Publication::Push).await;
}

async fn refusals(publication: Publication) {
    let site = Site::live().await;
    sqlx::query("UPDATE environments SET name='Private publication environment' WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let allow = json!([{"operation":publication.operation(),"actions":publication.actions()}]);
    for (user, selected, selection, why) in [
        (
            site.ada.id(),
            target("environment", new_id()),
            allow.clone(),
            "another environment",
        ),
        (
            site.ada.id(),
            target("thread", site.thread),
            allow.clone(),
            "the nearby thread gives no environment authority",
        ),
        (
            site.ada.id(),
            target("environment", site.environment),
            json!([{"operation":publication.operation(),"actions":["write"]}]),
            "Write cannot relabel publication Manage",
        ),
        (
            site.ada.id(),
            target("environment", site.environment),
            json!([{"operation":"EnvironmentSourcePrepare","actions":["manage"]}]),
            "preparing source does not grant commit or push",
        ),
        (
            site.vic.id(),
            target("environment", site.environment),
            allow,
            "the account still needs current FilesManage",
        ),
    ] {
        let issued = key(&site, user, selected, selection).await;
        let reply = publication.request(&site, &issued.secret).await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{why}: {}",
            String::from_utf8_lossy(&reply.raw)
        );
        if matches!(publication, Publication::Commit) {
            assert!(
                !String::from_utf8_lossy(&reply.raw).contains("Private publication environment"),
                "a Manage-only commit refusal contains no existing resource name: {why}"
            );
            let problem: Json = serde_json::from_slice(&reply.raw).unwrap();
            assert!(
                problem.get("missing").is_none(),
                "a Manage-only commit refusal never projects the named permission page"
            );
        }
    }
    if matches!(publication, Publication::Push) {
        for selection in [
            json!([{"operation":"EnvironmentGitPush","actions":["manage"]}]),
            json!([
                {"operation":"EnvironmentGitPush","actions":["manage"]},
                {"operation":"EnvironmentGitStatusGet","actions":["read"]}
            ]),
        ] {
            let issued = key(
                &site,
                site.ada.id(),
                target("environment", site.environment),
                selection,
            )
            .await;
            let reply = publication.request(&site, &issued.secret).await;
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "publication needs its own declared source Read: {}",
                String::from_utf8_lossy(&reply.raw)
            );
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind IN ('git_commit','git_push')",
    )
    .bind(site.environment)
    .fetch_one(&mut *site.world.conn().await)
    .await
    .unwrap();
    assert_eq!(count, 0, "a refused origin creates no publication job");
    assert_eq!(
        site.sent(orch_link_contract::commands::CommandKind::GitCommit),
        0
    );
    assert_eq!(
        site.sent(orch_link_contract::commands::CommandKind::GitPush),
        0
    );
    let ordinary = publication.request(&site, &site.ada.token).await;
    assert_eq!(
        ordinary.status,
        StatusCode::ACCEPTED,
        "ordinary browser authority remains sufficient: {}",
        String::from_utf8_lossy(&ordinary.raw)
    );
    let shown: Json = serde_json::from_slice(&ordinary.raw).unwrap();
    assert_eq!(
        shown["created_by"]["id"],
        json!(site.ada.id()),
        "browser keeps its existing full queued view"
    );
}

#[tokio::test]
async fn commit_keeps_operation_action_target_and_account_limits_and_browser_control() {
    refusals(Publication::Commit).await;
}

#[tokio::test]
async fn push_keeps_operation_action_target_and_account_limits_and_browser_control() {
    refusals(Publication::Push).await;
}

async fn legacy(site: &Site, scopes: &[String], node: Option<Id>) -> Key {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *site.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: site.ada.id(),
            name: "Ordinary Git permission key",
            token_hash: &token_hash(&secret),
            scopes,
            node_id: node,
            at: site.world.now(),
            expires_at: Some(site.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    Key { id, secret }
}

#[tokio::test]
async fn ordinary_permission_keys_keep_publication_authority_and_existing_standalone_resource_bounds()
 {
    for publication in [Publication::Commit, Publication::Push] {
        let site = Site::live().await;
        let read = legacy(&site, &["files.view".into()], None).await;
        assert_eq!(
            publication.request(&site, &read.secret).await.status,
            StatusCode::FORBIDDEN,
            "the account's own FilesManage cannot expand a legacy Read credential"
        );
        let bounded = legacy(&site, &["*".into()], Some(site.topic)).await;
        assert_eq!(
            publication.request(&site, &bounded.secret).await.status,
            StatusCode::FORBIDDEN,
            "a legacy node subtree still cannot use a standalone environment endpoint"
        );
        let ordinary = legacy(&site, &["*".into()], None).await;
        let reply = publication.request(&site, &ordinary.secret).await;
        assert_eq!(
            reply.status,
            StatusCode::ACCEPTED,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let shown: Json = serde_json::from_slice(&reply.raw).unwrap();
        assert_eq!(shown["created_by"]["id"], json!(site.ada.id()));
        assert_eq!(shown["state"], "queued");
        let id = serde_json::from_value(shown["id"].clone()).unwrap();
        let row = jobs::job(&mut *site.world.conn().await, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.plan["token"], json!(ordinary.id));
        assert_eq!(row.environment_id, Some(site.environment));
    }
}
