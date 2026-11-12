//! Temporary conversation boundaries compose with the ordinary page evaluator.
use crate::Error;
use orch_db::{Conn, queries::ask::Governance};
use orch_types::{Id, Time};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct Boundary {
    pub root: Id,
    pub person: Id,
    pub expires: Option<Time>,
}

fn setting_error(error: orch_settings::Error) -> Error {
    match error {
        orch_settings::Error::Db(error) => Error::Db(error),
        error => Error::Stored(error.to_string()),
    }
}

pub(crate) async fn from_rows(
    conn: &mut Conn,
    rows: Vec<Governance>,
) -> Result<HashMap<Id, Boundary>, Error> {
    let mut bounds = HashMap::new();
    let mut out = HashMap::new();
    for Governance {
        thread_id,
        root_org_id,
        person_id,
        retention_days,
        last_message_at,
        root_live,
    } in rows
    {
        let expires = if root_live {
            let (minimum, maximum) = match bounds.get(&root_org_id).copied() {
                Some(bounds) => bounds,
                None => {
                    let minimum = orch_settings::get::<i64>(
                        conn,
                        orch_settings::Place::Node(root_org_id),
                        "ask.retention_min_days",
                    )
                    .await
                    .map_err(setting_error)?
                    .unwrap_or(1);
                    let maximum = orch_settings::get::<i64>(
                        conn,
                        orch_settings::Place::Node(root_org_id),
                        "ask.retention_max_days",
                    )
                    .await
                    .map_err(setting_error)?
                    .unwrap_or(3650);
                    bounds.insert(root_org_id, (minimum, maximum));
                    (minimum, maximum)
                }
            };
            if minimum < 1 || maximum < minimum {
                None
            } else {
                chrono::Duration::try_days(i64::from(retention_days).clamp(minimum, maximum))
                    .and_then(|days| last_message_at.checked_add_signed(days))
                    .map(Time::new)
            }
        } else {
            None
        };
        out.insert(
            thread_id,
            Boundary {
                root: root_org_id,
                person: person_id,
                expires,
            },
        );
    }
    Ok(out)
}
