//! Native administered-place window, with scope applied before its existing page boundary.

use super::*;

pub use orch_api_base::routes::ports::outside_operation::ENV;

pub use orch_api_base::routes::ports::outside_operation::HOST;

pub use orch_api_base::routes::ports::outside_operation::MACHINE;

pub use orch_api_base::routes::ports::outside_operation::NAMES;

pub use orch_api_base::routes::ports::outside_operation::Outside;

pub use orch_api_base::routes::ports::outside_operation::Selected;

pub use orch_api_base::routes::ports::outside_operation::Opening;

pub use orch_api_base::routes::ports::outside_operation::collection;

pub use orch_api_base::routes::ports::outside_operation::read_for_overview;

pub use orch_api_base::routes::ports::outside_operation::select_in;
async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    ctx.ordinary_node_administration(conn, principal, ctx.id("id")?)
        .await
}
orch_api_base::__api_bind_66efe983d9!();

async fn render_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Selected,
) -> ApiResult<Page<PortOpening>> {
    let mut items = Vec::new();
    for selected in value.rows.iter().filter(|row| row.show) {
        items.push(
            opening_view_prepared(
                ctx,
                conn,
                principal,
                &selected.row,
                &selected.environment,
                false,
                Some(&value.names),
                Some(selected.state.clone()),
            )
            .await?,
        );
    }
    Ok(Page {
        items,
        next_cursor: value.next_cursor.clone(),
    })
}
orch_api_base::__api_bind_daec2f128a!();
