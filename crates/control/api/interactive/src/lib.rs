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
    pub mod browsers;
    pub mod displays;
    pub mod gateways;
    pub mod interactive_close;
    pub mod interactive_creation;
    pub mod interactive_tickets;
    pub mod meta;
    pub mod port_entry;
    pub mod port_requests;
    pub mod ports;
    pub mod terminals;
    pub mod usage;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::browsers::__api_install_ea12023e1e();

    routes::browsers::__api_install_0ebb32af85();

    routes::browsers::__api_install_3f4b14d8fe();

    routes::browsers::__api_install_66bd6d39aa();

    routes::displays::__api_install_0102d264ad();

    routes::displays::__api_install_c12234bdba();

    routes::displays::__api_install_3e10c98a8c();

    routes::gateways::__api_install_c61b72f407();

    routes::gateways::__api_install_d8b3eca981();

    routes::interactive_tickets::__api_install_caa4e5bd2d();

    routes::meta::__api_install_2af41ef4f9();

    routes::meta::__api_install_ec9f465d00();

    routes::meta::__api_install_f9d963c9b2();

    routes::meta::__api_install_f02731996c();

    routes::port_entry::__api_install_36aa4772f8();

    routes::port_entry::__api_install_7dd1829dd2();

    routes::port_requests::__api_install_386184d107();

    routes::port_requests::__api_install_6669de2a5f();

    routes::port_requests::__api_install_e7861bd2fa();

    routes::port_requests::__api_install_79a50b72b8();

    routes::ports::opening_operation::__api_install_40d114e821();

    routes::ports::opening_operation::__api_install_f02d1b6741();

    routes::ports::outside_operation::__api_install_66efe983d9();

    routes::ports::outside_operation::__api_install_daec2f128a();

    routes::ports::__api_install_aa4ff1ee37();

    routes::ports::__api_install_3a27b86d0a();

    routes::ports::__api_install_649f5bdf2b();

    routes::ports::__api_install_38c34fdc8f();

    routes::ports::__api_install_9bf1cd7659();

    routes::ports::__api_install_3dde7fe764();

    routes::ports::__api_install_9473b2289c();

    routes::ports::__api_install_31c1d582af();

    routes::ports::__api_install_3d55c330f8();

    routes::ports::__api_install_b73cd65628();

    routes::ports::__api_install_f62f18bc58();

    routes::ports::__api_install_d4842200b4();

    routes::ports::__api_install_b89f1ee526();

    routes::ports::__api_install_a8fb13606b();

    routes::ports::__api_install_1691768fc2();

    routes::ports::__api_install_aef3097442();

    routes::terminals::__api_install_26765e8efb();

    routes::terminals::__api_install_09bdb1d5b2();

    routes::usage::caps::__api_install_3887dbdd7a();

    routes::usage::caps::__api_install_e13c9f148f();

    routes::usage::caps::__api_install_ef3be4443e();

    routes::usage::caps::__api_install_dddf635d8b();

    routes::usage::caps::__api_install_917524335b();

    routes::usage::caps::__api_install_285fabea4f();

    routes::usage::events::__api_install_87631274e1();

    routes::usage::figures::__api_install_28d6ff760e();

    routes::usage::figures::__api_install_a3e19cca5d();

    routes::usage::figures::__api_install_2f2f8642a0();

    routes::usage::figures::__api_install_e9ec524ffe();

    routes::usage::figures::__api_install_e5b5427b0c();

    routes::usage::figures::__api_install_a33c56a037();

    routes::usage::figures::__api_install_c62d9b5510();

    routes::usage::figures::__api_install_fc2b3630df();

    routes::usage::figures::__api_install_cb3cbe2f30();

    routes::usage::names::__api_install_fd9e5a14be();

    routes::usage::native::__api_install_37016ebbd9();

    routes::usage::scope::__api_install_3c5645224e();

    routes::usage::scope::__api_install_5f74f1b8b7();

    routes::usage::scope::__api_install_2ce58f8e2a();

    routes::usage::scope::__api_install_d76bd083b9();

    routes::usage::scope::__api_install_0339046321();

    routes::usage::scope::__api_install_86c6ad7e57();

    routes::usage::scope::__api_install_ddbe793671();

    routes::usage::__api_install_641a976536();
}
