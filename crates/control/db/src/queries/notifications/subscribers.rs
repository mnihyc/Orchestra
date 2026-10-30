//! Who follows a place and which destinations are told of it: the nearest subscription of each person on the way down to a node, the automatic row of whoever owns or writes in a thread, and the destinations set at a node or above it.
//!
//! A person's level at a node is the level of their nearest subscription on
//! the way from the node up to its root org, `off` where they have none. So
//! a bell on a project is followed by every topic and thread below it until a
//! nearer row says otherwise, and a row of `off` on a thread silences a
//! project that is followed.

use super::channels::DestinationRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// A person and their level at a node, which a subscription of theirs decides.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Follower {
    /// The person.
    pub user_id: Id,
    /// `all`, `important` or `off`.
    pub level: String,
}

/// Every person who has a subscription at `node` or at a node above it, with
/// the level of the nearest one. Nobody is listed twice.
pub async fn followers_of(conn: &mut Conn, node: Id) -> Result<Vec<Follower>, DbError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (s.user_id) s.user_id, s.level
           FROM nodes n
          CROSS JOIN LATERAL unnest(n.path) WITH ORDINALITY AS p(node_id, ord)
           JOIN subscriptions s ON s.node_id = p.node_id
          WHERE n.id = $1
          ORDER BY s.user_id, p.ord DESC",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// The level a person has at `node`: that of their nearest subscription on the
/// way up, `off` where they have none.
pub async fn level_of(conn: &mut Conn, user: Id, node: Id) -> Result<String, DbError> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT s.level
           FROM nodes n
          CROSS JOIN LATERAL unnest(n.path) WITH ORDINALITY AS p(node_id, ord)
           JOIN subscriptions s ON s.node_id = p.node_id AND s.user_id = $2
          WHERE n.id = $1
          ORDER BY p.ord DESC
          LIMIT 1",
    )
    .bind(node)
    .bind(user)
    .fetch_optional(conn)
    .await?;
    Ok(found.unwrap_or_else(|| "off".to_owned()))
}

/// Makes a person follow a node at `all` by an automatic row, because they own
/// it or wrote in it. A person who has a row at that node already, whichever
/// they set it to, keeps theirs, and a service identity, which is told
/// nothing, follows nothing. Says whether the row was written. The person's
/// open screens are not sent news of it: a client loads a person's levels when
/// it connects and when it opens a place, and the row is there by then.
pub async fn follow(
    conn: &mut Conn,
    user: Id,
    node: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO subscriptions (user_id, node_id, level, source, updated_at)
         SELECT u.id, $2, 'all', 'auto', $3 FROM users u WHERE u.id = $1 AND u.kind = 'human'
         ON CONFLICT (user_id, node_id) DO NOTHING",
    )
    .bind(user)
    .bind(node)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Which of these are people, as against service identities, which are told
/// nothing: integrations use destinations.
pub async fn people_among(conn: &mut Conn, users: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar(
            "SELECT id FROM users WHERE id = ANY($1) AND kind = 'human' ORDER BY id",
        )
        .bind(users)
        .fetch_all(conn)
        .await?,
    )
}

/// The first owner of a node; none where there is no such node.
pub async fn owner_of(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id = $1")
            .bind(node)
            .fetch_optional(conn)
            .await?,
    )
}

/// The nodes from the root org down to `node`, itself last; empty where there
/// is no such node.
pub async fn path_of(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    let path: Option<Vec<Id>> = sqlx::query_scalar("SELECT path FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(conn)
        .await?;
    Ok(path.unwrap_or_default())
}

/// The destinations that are in use at the nodes of `path`, the root org's
/// first and then those further down, each group in the order it was set up.
pub async fn destinations_on(conn: &mut Conn, path: &[Id]) -> Result<Vec<DestinationRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.id, d.node_id, d.channel, d.name, d.config, d.templates, d.kinds, d.level, d.enabled,
                d.last_ok_at, d.last_error, d.created_by, d.created_at, d.updated_at
           FROM destinations d
           JOIN unnest($1::uuid[]) WITH ORDINALITY AS p(node_id, ord) ON p.node_id = d.node_id
          WHERE d.enabled
          ORDER BY p.ord, d.created_at, d.id",
    )
    .bind(path)
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::hierarchy::{put_subscription, testing::insert_node};
    use crate::queries::identity::testing::insert_user;
    use crate::queries::notifications::channels::{NewDestination, insert_destination};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    struct Places {
        org: Id,
        project: Id,
        topic: Id,
    }

    async fn person(conn: &mut Conn, name: &str) -> Id {
        let id = new_id();
        insert_user(conn, id, name).await.unwrap();
        id
    }

    async fn places(conn: &mut Conn, owner: Id) -> Places {
        let (org, project, topic) = (new_id(), new_id(), new_id());
        insert_node(conn, org, "org", None, "Acme", owner)
            .await
            .unwrap();
        insert_node(conn, project, "project", Some(org), "Web", owner)
            .await
            .unwrap();
        insert_node(conn, topic, "topic", Some(project), "Login", owner)
            .await
            .unwrap();
        Places {
            org,
            project,
            topic,
        }
    }

    #[tokio::test]
    async fn a_person_follows_a_place_at_the_level_of_the_nearest_row_on_the_way_up() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bob, cyd) = (
            person(&mut conn, "ada").await,
            person(&mut conn, "bob").await,
            person(&mut conn, "cyd").await,
        );
        let at = places(&mut conn, ada).await;
        let now = clock::start();
        for (who, node, level) in [
            (ada, at.org, "all"),
            (bob, at.org, "all"),
            (bob, at.topic, "off"),
            (cyd, at.project, "important"),
        ] {
            put_subscription(&mut conn, who, node, level, "manual", now)
                .await
                .unwrap();
        }

        let mut at_topic: Vec<(Id, String)> = followers_of(&mut conn, at.topic)
            .await
            .unwrap()
            .into_iter()
            .map(|each| (each.user_id, each.level))
            .collect();
        at_topic.sort();
        let mut expected = vec![
            (ada, "all".to_owned()),
            (bob, "off".to_owned()),
            (cyd, "important".to_owned()),
        ];
        expected.sort();
        assert_eq!(at_topic, expected, "a nearer row decides, off included");
        let at_project: Vec<_> = followers_of(&mut conn, at.project)
            .await
            .unwrap()
            .into_iter()
            .filter(|each| each.user_id == bob)
            .map(|each| each.level)
            .collect();
        assert_eq!(at_project, ["all"], "a row below is not followed above");
        assert_eq!(level_of(&mut conn, bob, at.topic).await.unwrap(), "off");
        assert_eq!(
            level_of(&mut conn, cyd, at.topic).await.unwrap(),
            "important"
        );
        assert_eq!(
            level_of(&mut conn, cyd, at.org).await.unwrap(),
            "off",
            "nobody follows what they have no row at or above"
        );
    }

    #[tokio::test]
    async fn an_automatic_row_never_replaces_the_persons_own_and_a_service_identity_follows_nothing()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bot) = (
            person(&mut conn, "ada").await,
            person(&mut conn, "ci-bot").await,
        );
        let at = places(&mut conn, ada).await;
        sqlx::query("UPDATE users SET kind = 'service', service_org_id = $2 WHERE id = $1")
            .bind(bot)
            .bind(at.org)
            .execute(&mut *conn)
            .await
            .unwrap();
        let now = clock::start();
        assert!(follow(&mut conn, ada, at.topic, now).await.unwrap());
        assert!(
            !follow(&mut conn, ada, at.topic, now).await.unwrap(),
            "a row that is there is left alone"
        );
        put_subscription(&mut conn, ada, at.project, "off", "manual", now)
            .await
            .unwrap();
        assert!(!follow(&mut conn, ada, at.project, now).await.unwrap());
        assert_eq!(level_of(&mut conn, ada, at.project).await.unwrap(), "off");
        assert!(!follow(&mut conn, bot, at.topic, now).await.unwrap());
        assert_eq!(
            people_among(&mut conn, &[ada, bot]).await.unwrap(),
            [ada],
            "told to people"
        );
    }

    #[tokio::test]
    async fn the_destinations_on_a_way_are_those_in_use_from_the_root_down_in_the_order_they_were_made()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = person(&mut conn, "ada").await;
        let at = places(&mut conn, ada).await;
        let now = clock::start();
        let made = [
            (at.topic, "topic room", true, 1),
            (at.org, "org room", true, 2),
            (at.project, "off room", false, 3),
            (at.project, "project room", true, 4),
        ];
        for (node, name, enabled, seconds) in made {
            insert_destination(
                &mut conn,
                &NewDestination {
                    id: new_id(),
                    node_id: node,
                    channel: "webhook",
                    name,
                    config: &json!({}),
                    templates: &json!({}),
                    kinds: &[],
                    level: "important",
                    enabled,
                    created_by: ada,
                    at: now + chrono::Duration::seconds(seconds),
                },
            )
            .await
            .unwrap();
        }
        let path = path_of(&mut conn, at.topic).await.unwrap();
        assert_eq!(path, [at.org, at.project, at.topic]);
        let found: Vec<String> = destinations_on(&mut conn, &path)
            .await
            .unwrap()
            .into_iter()
            .map(|each| each.name)
            .collect();
        assert_eq!(found, ["org room", "project room", "topic room"]);
        assert_eq!(
            destinations_on(&mut conn, &path[..2]).await.unwrap().len(),
            2,
            "what is set below the node is not for it"
        );
    }
}
