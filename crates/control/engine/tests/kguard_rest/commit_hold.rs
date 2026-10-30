//! Pause a real commit after its epoch checks, without replacing or changing those checks.

use crate::common::World;
use orch_types::Id;

// The fixture uses the two-integer advisory-lock namespace; production named
// locks use the separate single-bigint namespace. One isolated test database
// runs one prescribed interleaving at a time. 0x6b67 is the fixture's "kg" tag.
const LATCH_CLASS: i32 = 0x6b67;
const LATCH_SLOT: i32 = 1;

pub enum Target<'a> {
    Grant(Id),
    Machine(Id),
    Report(Id),
    Move(Id),
    Create { node: Id, title: &'a str },
}

pub async fn install(world: &World) {
    // orch_commit_work's final DELETE runs after the production epoch comparison
    // and counter writes. This observer only waits there; it grants no access,
    // changes no guard setting and never moves an epoch check to an earlier phase.
    let sql = r#"
        CREATE TABLE guard_rest_target(singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
            kind text NOT NULL, identity uuid, node uuid, title text);
        CREATE FUNCTION guard_rest_mark_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        DECLARE wanted guard_rest_target; body jsonb := to_jsonb(NEW); prior jsonb := to_jsonb(OLD);
        BEGIN
            SELECT * INTO wanted FROM guard_rest_target WHERE singleton;
            IF (TG_TABLE_NAME='permission_entries' AND wanted.kind='grant'
                    AND body->>'id'=wanted.identity::text AND body->>'removed_at' IS NOT NULL)
                OR (TG_TABLE_NAME='machines' AND wanted.kind='machine'
                    AND body->>'id'=wanted.identity::text AND body->>'status'='revoked')
                OR (TG_TABLE_NAME='documents' AND wanted.kind='move'
                    AND body->>'id'=wanted.identity::text AND body->>'node_id' IS DISTINCT FROM prior->>'node_id')
                OR (TG_TABLE_NAME='nodes' AND wanted.kind='report'
                    AND body->>'id'=wanted.identity::text AND body->>'name' IS DISTINCT FROM prior->>'name')
                OR (TG_TABLE_NAME='documents' AND wanted.kind='create' AND TG_OP='INSERT'
                    AND body->>'node_id'=wanted.node::text AND body->>'title'=wanted.title) THEN
                PERFORM set_config('guard_rest.pause_commit','on',true);
            END IF;
            RETURN NEW;
        END $$;
        CREATE TRIGGER guard_rest_mark_grant AFTER UPDATE ON permission_entries
            FOR EACH ROW EXECUTE FUNCTION guard_rest_mark_commit();
        CREATE TRIGGER guard_rest_mark_machine AFTER UPDATE ON machines
            FOR EACH ROW EXECUTE FUNCTION guard_rest_mark_commit();
        CREATE TRIGGER guard_rest_mark_document AFTER INSERT OR UPDATE ON documents
            FOR EACH ROW EXECUTE FUNCTION guard_rest_mark_commit();
        CREATE TRIGGER guard_rest_mark_report AFTER UPDATE ON nodes
            FOR EACH ROW EXECUTE FUNCTION guard_rest_mark_commit();
        CREATE FUNCTION guard_rest_pause_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF COALESCE(current_setting('guard_rest.pause_commit',true),'')='on' THEN
                PERFORM pg_advisory_xact_lock(27495,1);
            END IF;
            RETURN OLD;
        END $$;
        CREATE TRIGGER guard_rest_pause_commit BEFORE DELETE ON commit_work
            FOR EACH ROW EXECUTE FUNCTION guard_rest_pause_commit();
    "#;
    sqlx::raw_sql(sql).execute(world.pool.pool()).await.unwrap();
}

pub struct Hold {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
    pub pid: i32,
}

impl Hold {
    pub async fn at(world: &World, target: Target<'_>) -> Self {
        let (kind, identity, node, title) = match target {
            Target::Grant(id) => ("grant", Some(id), None, None),
            Target::Machine(id) => ("machine", Some(id), None, None),
            Target::Report(id) => ("report", Some(id), None, None),
            Target::Move(id) => ("move", Some(id), None, None),
            Target::Create { node, title } => ("create", None, Some(node), Some(title)),
        };
        sqlx::query("INSERT INTO guard_rest_target(singleton,kind,identity,node,title) VALUES(true,$1,$2,$3,$4)
            ON CONFLICT(singleton) DO UPDATE SET kind=EXCLUDED.kind,identity=EXCLUDED.identity,node=EXCLUDED.node,title=EXCLUDED.title")
            .bind(kind).bind(identity).bind(node).bind(title).execute(world.pool.pool()).await.unwrap();
        // This raw transaction is only the observer's latch, not an authority
        // transaction and not the action being proved.
        let mut tx = world.pool.pool().begin().await.unwrap();
        let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1,$2)")
            .bind(LATCH_CLASS)
            .bind(LATCH_SLOT)
            .execute(&mut *tx)
            .await
            .unwrap();
        Self { tx, pid }
    }

    pub async fn release(self) {
        self.tx.rollback().await.unwrap();
    }
}
