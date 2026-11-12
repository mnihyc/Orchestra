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
    pub mod environments;
    pub mod fleet;
    pub mod loss;
    pub mod processes;
    pub mod workspace;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::environments::exec_version::__api_install_754fbc9cee();

    routes::environments::hosted::__api_install_3664da9cf1();

    routes::environments::hosted::__api_install_25a1104266();

    routes::environments::hosted::__api_install_84fb7b9d00();

    routes::environments::hosted::__api_install_a292f8a126();

    routes::environments::hosted::__api_install_0df7f1c396();

    routes::environments::jobs::subject::__api_install_302f5be720();

    routes::environments::jobs::subject::__api_install_0f6e4906bc();

    routes::environments::jobs::__api_install_c328160ae9();

    routes::environments::jobs::__api_install_08462586ba();

    routes::environments::jobs::__api_install_e31437c66b();

    routes::environments::jobs::__api_install_1e8912e3ab();

    routes::environments::jobs::__api_install_5fe1762c39();

    routes::environments::mcp::create_sources::__api_install_a0c768943c();

    routes::environments::mcp::create_sources::__api_install_be38dce02e();

    routes::environments::mcp::__api_install_25bd166465();

    routes::environments::mcp::__api_install_6abcd777fa();

    routes::environments::moves::__api_install_4d665163fa();

    routes::environments::moves::__api_install_8e01278771();

    routes::environments::network::__api_install_4c76ea5c7c();

    routes::environments::network::__api_install_54063f987e();

    routes::environments::places::__api_install_0165bcdf3c();

    routes::environments::places::__api_install_84d36c5654();

    routes::environments::places::__api_install_a7a7119068();

    routes::environments::places::__api_install_08e2411db6();

    routes::environments::places::__api_install_804a7d87b5();

    routes::environments::places::__api_install_f09a39d7c1();

    routes::environments::places::__api_install_525d07a808();

    routes::environments::places::__api_install_1c0f1e698f();

    routes::environments::places::__api_install_e34cda7dae();

    routes::environments::places::__api_install_3cd543019e();

    routes::environments::places::__api_install_75ad864d9c();

    routes::environments::read_sources::__api_install_07c8ba3b15();

    routes::environments::reading::__api_install_50baf5598a();

    routes::environments::reading::__api_install_0794630518();

    routes::environments::reading::__api_install_f45b52f7df();

    routes::environments::reading::__api_install_cfc8a895c0();

    routes::environments::retained::__api_install_069be0bc4b();

    routes::environments::retained::__api_install_c4a5a90a7f();

    routes::environments::retained::__api_install_e0b0aea787();

    routes::environments::sync::__api_install_60d76202f7();

    routes::environments::sync::__api_install_4a2e170369();

    routes::environments::sync::__api_install_4b39fa92cc();

    routes::environments::updates::__api_install_c3cec1834a();

    routes::environments::updates::__api_install_e77c594372();

    routes::environments::updates::__api_install_bf2b3071af();

    routes::environments::updates::__api_install_3149c67600();

    routes::environments::updates::__api_install_c44e070c1c();

    routes::environments::updates::__api_install_8e42e7dfad();

    routes::environments::updates::__api_install_6ac5ae2c78();

    routes::environments::updates::__api_install_0daadb08b6();

    routes::environments::updates::__api_install_d91e721e63();

    routes::environments::__api_install_a7561acc31();

    routes::environments::__api_install_5e3042a2f8();

    routes::environments::__api_install_4c32840a49();

    routes::environments::__api_install_a51865252c();

    routes::environments::__api_install_1ae31b360e();

    routes::environments::__api_install_82ef5af6e4();

    routes::environments::__api_install_1feedd6762();

    routes::environments::__api_install_58797c68d7();

    routes::environments::__api_install_da2cee7ceb();

    routes::environments::__api_install_b143dd075a();

    routes::environments::__api_install_e075801632();

    routes::fleet::__api_install_005d526ddc();

    routes::loss::__api_install_34f314958c();

    routes::processes::__api_install_a36fdefcd0();

    routes::workspace::door::__api_install_f1c62865f5();

    routes::workspace::files::__api_install_c3e4d76c51();

    routes::workspace::files::__api_install_223f89e0d4();

    routes::workspace::live::__api_install_0a904728c5();

    routes::workspace::live::__api_install_30aad6c07a();

    routes::workspace::live::__api_install_190ad4e921();

    routes::workspace::live::__api_install_c89aa3c2c0();

    routes::workspace::pages::__api_install_5406945ec5();

    routes::workspace::paths::__api_install_4f59b857ad();

    routes::workspace::problems::__api_install_a1ebe93450();

    routes::workspace::__api_install_3431b4ebde();
}
