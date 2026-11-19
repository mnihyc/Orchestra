//! Typed native rich refusals use actual response sources and a same-error sparse fallback.

use super::*;

/// A failed native writer supplies its immutable native error/view input and existing renderer.
/// Its Projection declaration contains only actual known mandatory and protected error sources;
/// success-only singletons are absent, never guessed empty. No writer/initial IO is retried.
pub(crate) async fn project<F: reply::Projection<Reply = ApiError>>(
    ctx: &Ctx,
    attempted: &Caller,
    input: F::Input,
    sparse: ApiError,
) -> ApiResult<ApiError> {
    super::super::publication::refusal(ctx, &sparse)?;
    let reading = ctx.for_refusal_projection();
    match reply::render_refusal::<F>(&reading, attempted, input).await? {
        Some(rich) => {
            if !super::super::publication::same_refusal(&rich, &sparse) {
                return Err(ApiError::internal(
                    "native refusal projection changed its status, code or retry hint",
                ));
            }
            Ok(rich)
        }
        None => Ok(sparse),
    }
}
