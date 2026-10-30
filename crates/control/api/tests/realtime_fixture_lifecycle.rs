//! The real fixture joins its notice/socket owners before either pool or database is removed.

mod realtime_support;

use realtime_support::{Client, Rig};

#[tokio::test]
async fn successive_current_thread_fixtures_join_callbacks_and_close_both_pools_before_drop() {
    for _ in 0..3 {
        let rig = Rig::new().await;
        let application = rig.pool.clone();
        let database = rig.db.pool.clone();
        let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
        client.ping().await;
        assert_eq!(
            rig.server.task_owners(),
            2,
            "the HTTP server and native notice applier are owned"
        );
        rig.shutdown().await;
        assert_eq!(rig.server.task_owners(), 0);
        assert_eq!(rig.server.realtime.connections(), 0);
        assert_eq!(application.pool().size(), 0);
        assert!(application.pool().is_closed());
        assert_eq!(database.size(), 0);
        assert!(database.is_closed());
        rig.shutdown().await;
        drop(client);
        drop(rig);
        assert!(matches!(
            application.pool().acquire().await,
            Err(sqlx::Error::PoolClosed)
        ));
        assert!(matches!(
            database.acquire().await,
            Err(sqlx::Error::PoolClosed)
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_disconnect_is_joined_and_server_stop_does_not_close_a_sibling_process_pool() {
    let rig = Rig::new().await;
    let sibling = realtime_support::Server::over(
        &rig.pool,
        &rig.clock,
        realtime_support::quick(&rig.server.app.config),
    )
    .await;
    let mut client = Client::signed_in(sibling.addr, &rig.token).await;
    rig.machine.link.shutdown().await;
    rig.server.stop().await;
    assert_eq!(rig.server.task_owners(), 0);
    assert!(
        !rig.pool.pool().is_closed(),
        "stopping one process cannot close a shared database pool"
    );
    client.ping().await;
    sibling.stop().await;
    drop(client);
    drop(sibling);
    rig.shutdown().await;
    assert_eq!(rig.server.task_owners(), 0);
    assert!(rig.pool.pool().is_closed());
}
