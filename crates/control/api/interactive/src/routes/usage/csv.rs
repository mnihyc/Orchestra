//! The other form of an answer: comma-separated lines, escaped as RFC 4180 says, for the series of figures and the records of the ledger; and the JSON answer of a request that can give either.
//!
//! A request that gives either form is answered by hand, since the registry
//! writes JSON alone: the form is chosen by what the reader accepts.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use axum::response::IntoResponse;

use axum::response::Response;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use serde::Serialize;

use std::borrow::Cow;
/// The type of a CSV answer.
pub const CONTENT_TYPE: &str = "text/csv; charset=utf-8";

/// One data cell: formula-leading native text stays text when opened in a spreadsheet.
/// Signed finite numeric literals retain their exact bytes; classification never rounds the value.
#[must_use]
pub fn cell(text: &str) -> String {
    let start =
        text.trim_start_matches(|c: char| c.is_whitespace() || c.is_control() || c == '\u{feff}');
    let signed_number = matches!(start.chars().next(), Some('+' | '-'))
        && start.trim_end().parse::<f64>().is_ok_and(f64::is_finite);
    let formula = matches!(start.chars().next(), Some('=' | '+' | '-' | '@')) && !signed_number;
    let text = if formula {
        Cow::Owned(format!("'{text}"))
    } else {
        Cow::Borrowed(text)
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.into_owned()
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

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn native_text_cannot_become_a_spreadsheet_formula_even_behind_controls() {
        assert_eq!(cell("=1+1"), "'=1+1");
        assert_eq!(cell("+SUM(A1:A2)"), "'+SUM(A1:A2)");
        assert_eq!(cell("-SUM(A1:A2)"), "'-SUM(A1:A2)");
        assert_eq!(cell("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(cell(" \t=1+1"), "' \t=1+1");
        assert_eq!(cell("\u{feff}=1+1"), "'\u{feff}=1+1");
        assert_eq!(cell("\0=1+1"), "'\0=1+1");
        assert_eq!(cell("\r\n=1+1"), "\"'\r\n=1+1\"");
        assert_eq!(
            cell("=HYPERLINK(\"https://example.invalid\",\"view\")"),
            "\"'=HYPERLINK(\"\"https://example.invalid\"\",\"\"view\"\")\""
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_native_text_cannot_become_a_spreadsheet_formula_even_behind_controls() {
        self::native_text_cannot_become_a_spreadsheet_formula_even_behind_controls()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn statistics_keep_exact_numeric_bytes_and_ordinary_native_words() {
        for number in [
            "0",
            "-1",
            "+1",
            "-0.0300",
            "1e30",
            "-1e30",
            "9223372036854775807",
        ] {
            assert_eq!(cell(number), number);
        }
        assert_eq!(cell("A native refusal"), "A native refusal");
        assert_eq!(cell("source-id-42"), "source-id-42");
        assert_eq!(cell("'already text"), "'already text");
        assert_eq!(cell("-infinity"), "'-infinity");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_statistics_keep_exact_numeric_bytes_and_ordinary_native_words() {
        self::statistics_keep_exact_numeric_bytes_and_ordinary_native_words()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled() {
        assert_eq!(cell("plain"), "plain");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(line(&["a", "b,c", ""]), "a,\"b,c\",\r\n");
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
     {
        self::a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
    }
}
