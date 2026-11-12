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
    pub mod approvals;
    pub mod forks;
    pub mod links;
    pub mod share;
    pub mod threads;
    pub mod topics;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::approvals::operation::__api_install_d708c28645();

    routes::approvals::operation::__api_install_7b89426127();

    routes::approvals::operation::__api_install_083f6d23de();

    routes::approvals::operation::__api_install_95d9a090f1();

    routes::approvals::operation::__api_install_b636a30341();

    routes::approvals::__api_install_4c6f191e70();

    routes::approvals::__api_install_73d0126e74();

    routes::approvals::__api_install_8e136e74a4();

    routes::approvals::__api_install_8fa9886651();

    routes::forks::__api_install_e570164848();

    routes::forks::__api_install_020ffd9110();

    routes::links::__api_install_e3004a2033();

    routes::links::__api_install_66b5eee4f2();

    routes::links::__api_install_9a1ffcc6eb();

    routes::links::__api_install_fa613b3b55();

    routes::share::operation::__api_install_d61a4f1c27();

    routes::share::__api_install_b57b73724e();

    routes::share::__api_install_eb48af3518();

    routes::share::__api_install_2f2b473303();

    routes::share::__api_install_ffc7d657fe();

    routes::share::__api_install_da018a6af3();

    routes::threads::adopt::operation::__api_install_1280176ede();

    routes::threads::approval_items::__api_install_f30f174188();

    routes::threads::binding::__api_install_9aa49dd569();

    routes::threads::creation_operation::__api_install_2835df0f88();

    routes::threads::creation_operation::__api_install_5fe59f6144();

    routes::threads::creation_operation::__api_install_4ad2f8ac5f();

    routes::threads::creation_operation::__api_install_87a2ecaa32();

    routes::threads::creation_operation::__api_install_61ae43b5cf();

    routes::threads::make::__api_install_5ad940dfb4();

    routes::threads::make::__api_install_8a3eb8d954();

    routes::threads::make::__api_install_db6ac243da();

    routes::threads::native::__api_install_3e8b0c222d();

    routes::threads::uploads::__api_install_0658d2fd50();

    routes::threads::uploads::__api_install_02b2e50501();

    routes::threads::view::__api_install_a3c1635f6e();

    routes::threads::__api_install_6de5279cdb();

    routes::threads::__api_install_0eb3ad3c68();

    routes::threads::__api_install_d06fba3ddf();

    routes::threads::__api_install_a84e544ee2();

    routes::threads::__api_install_f0eaaf0710();

    routes::topics::announcements::__api_install_e136e90bd5();

    routes::topics::sync::__api_install_e57ac81d44();

    routes::topics::sync::__api_install_1ea126ee80();

    routes::topics::__api_install_0d167a273a();
}
