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
    pub mod admin;
    pub mod admin_messages;
    pub mod auth;
    pub mod directory;
    pub mod first_steps;
    pub mod personal;
    pub mod profiles;
    pub mod settings;
    pub mod users;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::admin::audit::__api_install_944d774ef1();

    routes::admin::catalogue::__api_install_3f8389ed5c();

    routes::admin::catalogue::__api_install_9718450093();

    routes::admin::catalogue::__api_install_891ba98c5e();

    routes::admin::catalogue::__api_install_2402bcfdbf();

    routes::admin::defaults::__api_install_1b7dfb75e1();

    routes::admin::__api_install_6e6c1a7061();

    routes::admin::__api_install_fbe3eb6f13();

    routes::admin::__api_install_49c603b860();

    routes::admin_messages::__api_install_796195948a();

    routes::auth::creation::__api_install_2e1ba4f348();

    routes::auth::tokens::__api_install_ff99c0b88b();

    routes::auth::__api_install_074cc200b5();

    routes::directory::__api_install_0502b165d2();

    routes::directory::__api_install_bb09bd2387();

    routes::directory::__api_install_43f6a1890c();

    routes::first_steps::__api_install_f635db2fdc();

    routes::first_steps::__api_install_712bb19250();

    routes::first_steps::__api_install_3b673ad843();

    routes::first_steps::__api_install_ed8b0e6ce1();

    routes::first_steps::__api_install_8bf6e24f02();

    routes::first_steps::__api_install_c22f2bb4e0();

    routes::first_steps::__api_install_293ce96b17();

    routes::personal::viewed::__api_install_d8907363a0();

    routes::personal::native::__api_install_a2018a5d59();

    routes::personal::__api_install_3346c39c88();

    routes::profiles::__api_install_82605ffc6d();

    routes::profiles::__api_install_6e5f32e3ad();

    routes::settings::readers::__api_install_e928a69f70();

    routes::settings::readers::__api_install_63a3158c5e();

    routes::settings::readers::__api_install_645b0cf6ac();

    routes::settings::readers::__api_install_29632770a2();

    routes::settings::readers::__api_install_e4bc8ef338();

    routes::settings::readers::__api_install_b00f576e8e();

    routes::settings::readers::__api_install_d846652dcd();

    routes::settings::__api_install_03d340b82b();

    routes::settings::__api_install_d9260d2301();

    routes::settings::__api_install_ddd9a09986();

    routes::settings::__api_install_84368f51ca();

    routes::settings::__api_install_d30c90eee1();

    routes::settings::__api_install_ee30f0eed4();

    routes::settings::__api_install_446c263a3e();

    routes::settings::__api_install_e6f0b9fe82();

    routes::users::__api_install_43faf5f5b7();
}
