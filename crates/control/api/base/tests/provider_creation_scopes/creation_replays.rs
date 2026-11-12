//! Native managed and machine provider creators share current account-scoped replay.
use super::*;
use orch_api_contract::common::IDEMPOTENCY_HEADER;

async fn call(world: &World, machine: Option<Id>, token: &str, key: Id, body: &Json) -> RawReply {
    if let Some(machine) = machine {
        world
            .app
            .call(MachineProviderCreate, &[&machine])
            .token(token)
            .header(IDEMPOTENCY_HEADER, &key.to_string())
            .raw_body(body)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(ProviderCreate, &[])
            .token(token)
            .header(IDEMPOTENCY_HEADER, &key.to_string())
            .raw_body(body)
            .send_bytes()
            .await
    }
}
#[tokio::test]
async fn accepted_provider_creations_replay_current_content_without_home_or_machine_write() {
    for on_machine in [false, true] {
        let world = world().await;
        let c = resources::company(&world).await;
        let machine = if on_machine {
            Some(directory(&world, &c.boss, c.acme).await)
        } else {
            None
        };
        let op = if on_machine {
            "MachineProviderCreate"
        } else {
            "ProviderCreate"
        };
        let name = "Once accepted native provider";
        let body = if on_machine {
            json!(MachineProviderCreateBody::Api {
                home_id: Some(c.acme),
                name: name.into(),
                upstreams: vec![Upstream {
                    protocol: "openai-responses".into(),
                    base_url: "https://api.example.com/v1".into(),
                    options: json!({})
                }],
                sources: None
            })
        } else {
            json!(api(c.acme, name))
        };
        let mut policy = vec![rule(op, "org", c.acme, "self", &["provider"], &["manage"])];
        if let Some(machine) = machine {
            policy.push(rule(
                op,
                "machine",
                machine,
                "self",
                &["machine"],
                &["write"],
            ));
        }
        let writer = key(&world, c.boss.id(), policy).await;
        let key_id = new_id();
        let first = call(&world, machine, &writer, key_id, &body).await;
        let id = receipt(&world, &first, name).await;
        let first: Json = serde_json::from_slice(&first.raw).unwrap();
        let accepted_revision = first["revision"].clone();
        let before_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        let recorded: Json =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(c.boss.id())
                .bind(key_id.to_string())
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(recorded["outcome"]["id"], json!(id));
        assert_eq!(recorded["outcome"]["data"]["revision"], accepted_revision);
        assert!(!recorded.to_string().contains(SECRET));
        assert!(
            recorded["outcome"]["data"]
                .as_object()
                .unwrap()
                .keys()
                .all(|key| ["revision", "warning"].contains(&key.as_str()))
        );
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
            .bind(token_hash(&writer))
            .bind(world.now())
            .execute(&world.db.pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE providers SET name='Current native provider',revision=revision+1 WHERE id=$1",
        )
        .bind(id)
        .execute(&world.db.pool)
        .await
        .unwrap();
        let reader = key(
            &world,
            c.boss.id(),
            vec![rule(op, "provider", id, "self", &["provider"], &["read"])],
        )
        .await;
        let replay = call(&world, machine, &reader, key_id, &body).await;
        assert_eq!(
            replay.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&replay.raw)
        );
        let replay: Json = serde_json::from_slice(&replay.raw).unwrap();
        assert_eq!(replay["id"], json!(id));
        assert_eq!(replay["name"], "Current native provider");
        assert_ne!(replay["revision"], accepted_revision);
        let unrelated = key(
            &world,
            c.boss.id(),
            vec![rule(
                "ProviderGet",
                "provider",
                id,
                "self",
                &["provider"],
                &["read"],
            )],
        )
        .await;
        let replay = call(&world, machine, &unrelated, key_id, &body).await;
        assert_eq!(replay.status, StatusCode::OK);
        let replay: Json = serde_json::from_slice(&replay.raw).unwrap();
        assert_eq!(replay["projection"], "receipt");
        assert_eq!(replay["provider_id"], json!(id));
        assert_eq!(replay["revision"], accepted_revision);
        assert!(
            !call(&world, machine, &reader, new_id(), &body)
                .await
                .status
                .is_success()
        );
        let after_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(before_jobs, after_jobs);
        let made: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='provider.created' AND object_id=$1",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(made, 1);
    }
}
