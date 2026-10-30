//! Node icon palette validation and the colour a new place inherits from its parent's settings.

use super::invalid;
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::nodes::Node;
use orch_api_contract::users::COLOR_COUNT;
use orch_db::Conn;
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys::NODES_NEW_COLOR;
use orch_types::Id;

/// The first palette index for node icons; avatars use zero-based indices instead.
const FIRST_COLOR: u32 = 1;

/// The colour a node's view currently says it has.
pub(super) fn current(node: &Node) -> Option<u32> {
    match node {
        Node::Org(node) => node.base.color,
        Node::Project(node) => node.base.color,
        Node::Topic(node) => node.base.color,
        Node::Thread(node) => node.base.color,
    }
}

/// The colour of its own a node's view currently says it has.
pub(super) fn current_own(node: &Node) -> Option<&str> {
    match node {
        Node::Org(node) => node.base.custom_color.as_deref(),
        Node::Project(node) => node.base.custom_color.as_deref(),
        Node::Topic(node) => node.base.custom_color.as_deref(),
        Node::Thread(node) => node.base.custom_color.as_deref(),
    }
}

/// How many hexadecimal digits a colour of its own has after its `#`: two for each of red, green and blue.
const OWN_COLOR_DIGITS: usize = 6;

/// Validates a colour of its own, `#rrggbb` in either case and kept in small letters, and refuses one given with a
/// palette colour in the same request: one replaces the other. Absent stays absent; null takes it away.
pub(super) fn own(
    color: Option<Option<u32>>,
    own: Option<Option<&str>>,
) -> ApiResult<Option<Option<String>>> {
    let Some(own) = own else {
        return Ok(None);
    };
    if color.is_some() {
        return Err(invalid(
            "custom_color",
            "give either a colour of the palette or one of its own, not both",
        ));
    }
    own.map(|text| {
        let well_formed = text.strip_prefix('#').is_some_and(|digits| {
            digits.len() == OWN_COLOR_DIGITS
                && digits.chars().all(|digit| digit.is_ascii_hexdigit())
        });
        if well_formed {
            Ok(text.to_ascii_lowercase())
        } else {
            Err(invalid("custom_color", "must be #rrggbb, or null"))
        }
    })
    .transpose()
    .map(Some)
}

/// Validates a patch colour and retains the distinction between absent and cleared.
pub(super) fn checked(color: Option<Option<u32>>) -> ApiResult<Option<Option<i16>>> {
    color
        .map(|color| {
            color
                .map(|color| {
                    if !(FIRST_COLOR..=COLOR_COUNT).contains(&color) {
                        return Err(invalid(
                            "color",
                            &format!("must be {FIRST_COLOR} to {COLOR_COUNT}, or null"),
                        ));
                    }
                    i16::try_from(color).map_err(ApiError::internal)
                })
                .transpose()
        })
        .transpose()
}

/// Sets a newly made place's colour from its parent's setting; threads keep state icons.
pub(super) async fn start(conn: &mut Conn, node: Id, parent: Option<Id>) -> ApiResult<()> {
    let Some(parent) = parent else {
        return Ok(());
    };
    if get::<String>(conn, Place::Node(parent), NODES_NEW_COLOR)
        .await
        .map_err(ApiError::internal)?
        .as_deref()
        == Some("random")
    {
        let color = rand::random_range(FIRST_COLOR..=COLOR_COUNT);
        orch_db::queries::hierarchy::presentation::initial_color(
            conn,
            node,
            i16::try_from(color).map_err(ApiError::internal)?,
        )
        .await?;
    }
    Ok(())
}
