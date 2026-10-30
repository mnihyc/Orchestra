//! Authorization invalidation and revocation checks.
//!
//! One number, the authorization epoch, moves every time something changes
//! that can change what anybody may do: a role, a row or an entry of a
//! page, a group or a membership, an owner, a parent, the archive, a
//! suspension, the flag of a system admin, or a service identity's reach.
//! Personal credentials use their person's row lock and a targeted commit
//! announcement instead of serializing every person's epoch writes.
//! Whatever was decided under an older epoch may be decided
//! again. Committed changes wake every server listening for authorization
//! changes; a cached decision also has to respect the end of its grants.

use crate::Error;
use orch_db::Conn;
use orch_db::queries::access;

/// The epoch now.
pub async fn current(conn: &mut Conn) -> Result<i64, Error> {
    Ok(access::epoch(conn).await?)
}

/// Moves the epoch on, in the transaction of the change that calls for it,
/// and returns it. A change moves it once, whatever it changes; the move is
/// announced by the database's own call, so that no writer can forget to.
pub async fn bump(conn: &mut Conn) -> Result<i64, Error> {
    Ok(access::bump_epoch(conn).await?)
}

/// What a connection keeps to know whether its last check still holds: the
/// epoch it was made under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watch {
    seen: i64,
}

impl Watch {
    /// Begins watching from the epoch now: the check the connection has
    /// just made was made under it.
    pub async fn begin(conn: &mut Conn) -> Result<Self, Error> {
        Ok(Self {
            seen: current(conn).await?,
        })
    }

    /// Begins from an epoch that is known.
    #[must_use]
    pub fn at(seen: i64) -> Self {
        Self { seen }
    }

    /// The epoch the last check was made under.
    #[must_use]
    pub fn seen(&self) -> i64 {
        self.seen
    }

    /// Whether the epoch moved since the last check. When it did, the
    /// watch is brought to the new epoch, and the caller checks again what
    /// it keeps open: a feed, a shell.
    pub async fn moved(&mut self, conn: &mut Conn) -> Result<bool, Error> {
        let now = current(conn).await?;
        let moved = now != self.seen;
        self.seen = now;
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::database::TestDb;

    #[tokio::test]
    async fn a_watch_notices_a_change_once_and_then_follows_the_epoch() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        assert_eq!(current(&mut conn).await.unwrap(), 0);
        let mut watch = Watch::begin(&mut conn).await.unwrap();
        assert!(!watch.moved(&mut conn).await.unwrap(), "nothing changed");
        assert_eq!(bump(&mut conn).await.unwrap(), 1);
        assert!(watch.moved(&mut conn).await.unwrap(), "a change was made");
        assert!(!watch.moved(&mut conn).await.unwrap(), "and is told once");
        assert_eq!(watch.seen(), 1);
        let mut late = Watch::at(0);
        assert!(late.moved(&mut conn).await.unwrap());
    }
}
