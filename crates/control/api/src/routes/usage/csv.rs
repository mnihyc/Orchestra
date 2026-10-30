//! The other form of an answer: comma-separated lines, escaped as RFC 4180 says, for the series of figures and the records of the ledger; and the JSON answer of a request that can give either.
//!
//! A request that gives either form is answered by hand, since the registry
//! writes JSON alone: the form is chosen by what the reader accepts.

use crate::middleware::requests::{ApiError, ApiResult};
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use serde::Serialize;

/// The type of a CSV answer.
pub const CONTENT_TYPE: &str = "text/csv; charset=utf-8";

/// One cell, quoted where it holds a comma, a quote or a line break.
#[must_use]
pub fn cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// One line of cells, ended as CSV ends a line.
#[must_use]
pub fn line<S: AsRef<str>>(cells: &[S]) -> String {
    let mut text = cells
        .iter()
        .map(|each| cell(each.as_ref()))
        .collect::<Vec<_>>()
        .join(",");
    text.push_str("\r\n");
    text
}

/// A CSV answer.
#[must_use]
pub fn response(body: String) -> Response {
    let mut answer = (StatusCode::OK, body).into_response();
    answer
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
    answer
}

/// A JSON answer.
pub fn json_response<T: Serialize>(body: &T, status: StatusCode) -> ApiResult<Response> {
    let bytes = serde_json::to_vec(body).map_err(ApiError::internal)?;
    let mut answer = (status, bytes).into_response();
    answer.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled() {
        assert_eq!(cell("plain"), "plain");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(line(&["a", "b,c", ""]), "a,\"b,c\",\r\n");
    }
}
