//! Internal route family implementation.
#![forbid(unsafe_code)]
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "the native test cases fail by panicking"
    )
)]

pub use orch_api_base::*;
#[cfg(any(test, feature = "test-support"))]
extern crate self as orch_api_core;
pub mod routes {
    pub use orch_api_base::routes::*;
    pub mod document_tree;
    pub mod documents;
    pub mod downloads;
    pub mod exports;
    pub mod imports;
    pub mod storage;
    pub mod summaries;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::document_tree::metadata::__api_install_1e70f78bc0();

    routes::document_tree::metadata::__api_install_bae5735b23();

    routes::document_tree::metadata::__api_install_838bfdc53f();

    routes::document_tree::metadata::__api_install_d13bbf4a7d();

    routes::document_tree::metadata::__api_install_917e2cb467();

    routes::document_tree::metadata::__api_install_a77a8d0bbe();

    routes::document_tree::metadata::__api_install_e2a51ca39a();

    routes::document_tree::__api_install_f1635129b9();

    routes::document_tree::__api_install_b705bab7c6();

    routes::document_tree::__api_install_d61ba637ac();

    routes::documents::build::__api_install_5c43bf1498();

    routes::documents::build::__api_install_de17b89813();

    routes::documents::build::__api_install_8529c110e3();

    routes::documents::build::__api_install_e7a615ba5e();

    routes::documents::build::__api_install_1fc6486326();

    routes::documents::build::__api_install_b5e18a5d64();

    routes::documents::build::__api_install_839e75b1a6();

    routes::documents::export::__api_install_cfb69bd732();

    routes::documents::export::__api_install_2b048ad610();

    routes::documents::limits::__api_install_920f34e543();

    routes::documents::metadata_projection::__api_install_205f3cab3b();

    routes::documents::page::__api_install_cb888d4d4b();

    routes::documents::page::__api_install_33e74fbd40();

    routes::documents::read_operation::__api_install_3b390eb0a3();

    routes::documents::read_operation::__api_install_25013cd151();

    routes::documents::references::__api_install_22d35a3720();

    routes::documents::__api_install_00fe765ca7();

    routes::downloads::__api_install_e20b139d4b();

    routes::downloads::__api_install_f377bccefd();

    routes::downloads::__api_install_1bf4f6bf88();

    routes::exports::__api_install_1047e2b49e();

    routes::exports::__api_install_351ca3187e();

    routes::imports::__api_install_9586d57c59();

    routes::storage::__api_install_9fa9b3986a();

    routes::summaries::embedded::__api_install_e93ed15ce0();

    routes::summaries::embedded::__api_install_7e5709b692();

    routes::summaries::embedded::__api_install_face5ea742();

    routes::summaries::__api_install_862439cb84();

    routes::summaries::__api_install_2f44c829e4();
}
