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
    pub mod capabilities;
    pub mod duplicates;
    pub mod resources;
    pub mod trust;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::capabilities::__api_install_51c3f443d6();

    routes::duplicates::moving::__api_install_5ca59db85d();

    routes::duplicates::moving::__api_install_394be8c8e0();

    routes::duplicates::moving::__api_install_9dfaded361();

    routes::duplicates::__api_install_fa1774d3f2();

    routes::duplicates::__api_install_a8c9718aea();

    routes::duplicates::__api_install_6f65f9e908();

    routes::duplicates::__api_install_e598983ef2();

    routes::duplicates::__api_install_696be1c924();

    routes::duplicates::__api_install_f983cce395();

    routes::duplicates::__api_install_cdacc514fb();

    routes::duplicates::__api_install_29e0fce0ab();

    routes::duplicates::__api_install_2078e648bd();

    routes::duplicates::__api_install_0a665b42ba();

    routes::duplicates::__api_install_6d7f4bf1fb();

    routes::duplicates::__api_install_de7dea446f();

    routes::duplicates::__api_install_0fea42175e();

    routes::duplicates::__api_install_090d6c0668();

    routes::resources::agents::__api_install_005a5bf42e();

    routes::resources::agents::__api_install_2a26470ecc();

    routes::resources::agents::__api_install_51d577fc2d();

    routes::resources::agents::__api_install_e6e77546c5();

    routes::resources::context::__api_install_4961d39df5();

    routes::resources::context::__api_install_35df7b5ec1();

    routes::resources::context::__api_install_19bde9b549();

    routes::resources::context::__api_install_75a6668c1c();

    routes::resources::context::__api_install_0f877a22b0();

    routes::resources::context::__api_install_b57f3f3ce1();

    routes::resources::context::__api_install_e0bd87e4b3();

    routes::resources::creation_home::__api_install_4796f73651();

    routes::resources::environment_fleet::__api_install_da9ef22327();

    routes::resources::environment_places::__api_install_5045ede9eb();

    routes::resources::environment_read::__api_install_2caaba278c();

    routes::resources::host_delete::__api_install_9dd54ad790();

    routes::resources::hosts::__api_install_abd603b8b5();

    routes::resources::hosts::__api_install_7a74f15f7b();

    routes::resources::hosts::__api_install_e32b4d7a69();

    routes::resources::hosts::__api_install_13e7353061();

    routes::resources::hosts::__api_install_795fa7a950();

    routes::resources::hosts::__api_install_60ce0f2a85();

    routes::resources::hosts::__api_install_5f012c09b2();

    routes::resources::hosts::__api_install_5b3da33bcf();

    routes::resources::hosts::__api_install_2109b1c7fb();

    routes::resources::hosts::__api_install_8c15bb6b04();

    routes::resources::library::__api_install_eead74b020();

    routes::resources::machines::enrollment::__api_install_8fafa279ab();

    routes::resources::machines::__api_install_c5ec983f9e();

    routes::resources::machines::__api_install_55c8b9c49d();

    routes::resources::machines::__api_install_a909d67302();

    routes::resources::machines::__api_install_b444ed81ee();

    routes::resources::machines::__api_install_63d7556e7e();

    routes::resources::machines::__api_install_c297297595();

    routes::resources::machines::__api_install_d2ace19f11();

    routes::resources::machines::__api_install_cb869fa787();

    routes::resources::mcp::__api_install_2d568dc9e3();

    routes::resources::mcp::projection::__api_install_86dad5b667();

    routes::resources::mcp::projection::__api_install_c00043a98d();

    routes::resources::mcp::projection::__api_install_b71d1e0316();

    routes::resources::mcp::__api_install_b46eced664();

    routes::resources::mcp::__api_install_576be5d12f();

    routes::resources::mcp::__api_install_87ebeaa552();

    routes::resources::mcp::__api_install_0269d3f504();

    routes::resources::mcp::__api_install_35059fd8e4();

    routes::resources::mcp::__api_install_690315d601();

    routes::resources::mcp_readers::__api_install_92ce626e66();

    routes::resources::mcp_readers::__api_install_712f4c3049();

    routes::resources::mounts::__api_install_dd4e30c19a();

    routes::resources::mounts::__api_install_66bcfd9efe();

    routes::resources::mounts::__api_install_1cbe1308fe();

    routes::resources::page::__api_install_d4b53ba781();

    routes::resources::page::__api_install_26051412f4();

    routes::resources::projection::single::__api_install_227c63a964();

    routes::resources::projection::single::__api_install_dbb26ef146();

    routes::resources::projection::single::__api_install_01cdf1c787();

    routes::resources::projection::single::__api_install_ec4d5a12fd();

    routes::resources::projection::single::__api_install_07ede148ea();

    routes::resources::projection::__api_install_6110f20d78();

    routes::resources::projection::__api_install_f101738fc4();

    routes::resources::projection::__api_install_19ca020a5c();

    routes::resources::provider_recovery::__api_install_da1c5574f6();

    routes::resources::providers::__api_install_0f1e4ccdb2();

    routes::resources::providers::__api_install_1b8aab182e();

    routes::resources::providers::__api_install_23f24917ee();

    routes::resources::providers::__api_install_9be6fd1d28();

    routes::resources::providers::__api_install_4494adecd2();

    routes::resources::providers::__api_install_420c6a586c();

    routes::resources::providers::__api_install_e9b9616032();

    routes::resources::sealing::__api_install_2238e563fe();

    routes::resources::sealing::__api_install_f53aec78ac();

    routes::resources::skills::__api_install_d3042354bd();

    routes::resources::skills::__api_install_b0aaad3905();

    routes::resources::skills::__api_install_a11b3ea78e();

    routes::resources::skills::__api_install_c23fe61081();

    routes::resources::skills::__api_install_00c9851850();

    routes::resources::templates::__api_install_46453f6116();

    routes::resources::templates::__api_install_9924a0b621();

    routes::resources::templates::__api_install_9e6441beea();

    routes::resources::templates::__api_install_d83d9b6fff();

    routes::resources::validation::__api_install_d2a05682ae();

    routes::resources::validation::__api_install_ef3cd14b55();

    routes::resources::view::__api_install_9b9d72ac9f();

    routes::resources::view::__api_install_78f49040ef();

    routes::resources::view::__api_install_4354480158();

    routes::resources::view::__api_install_1d9f1e7836();

    routes::resources::view::__api_install_2708eb6c6a();

    routes::resources::view::__api_install_05cca12189();

    routes::resources::view::__api_install_7577050e26();

    routes::resources::view::__api_install_42fb982f58();

    routes::resources::view::__api_install_cddd9f4808();

    routes::resources::view::__api_install_98d591f9cc();

    routes::resources::view::__api_install_a613953738();

    routes::resources::view::__api_install_df4d6a0d97();

    routes::resources::__api_install_b76d4af877();

    routes::resources::__api_install_8cb26d9e4c();

    routes::resources::__api_install_08053fcda3();

    routes::resources::__api_install_6286b1a27a();

    routes::resources::__api_install_824140da0a();

    routes::trust::__api_install_edc9d609ea();

    routes::trust::__api_install_ec0f96e79d();
}
