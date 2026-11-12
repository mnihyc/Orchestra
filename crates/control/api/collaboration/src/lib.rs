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
    pub mod access;
    pub mod chats;
    pub mod destinations;
    pub mod everything;
    pub mod nodes;
    pub mod notifications;
    pub mod search;
    pub mod trace;
    pub mod transfer;
}

#[doc(hidden)]
pub fn install_interfaces() {
    routes::access::link_origin::__api_install_559437e4dc();

    routes::access::operations::__api_install_2e84a8704d();

    routes::access::operations::__api_install_b58b9c2537();

    routes::access::operations::__api_install_009a849538();

    routes::access::operations::__api_install_9eac307708();

    routes::access::operations::__api_install_237170b96a();

    routes::access::operations::__api_install_5e37b11ede();

    routes::access::page::__api_install_38b4b9df26();

    routes::access::page::__api_install_68981fb735();

    routes::access::page::__api_install_7a87b2e19a();

    routes::access::page::__api_install_2a8d78de40();

    routes::access::page::__api_install_f2ec241b11();

    routes::access::page::__api_install_d24bfacb4d();

    routes::access::page::__api_install_2632b760ea();

    routes::access::page::__api_install_bd744fe3cb();

    routes::access::page::__api_install_3af1ff1c83();

    routes::access::page::__api_install_88985d68e3();

    routes::access::page_operations::__api_install_82ef4ec4cc();

    routes::access::page_operations::__api_install_fe020d0482();

    routes::access::page_operations::__api_install_cde21c68a9();

    routes::access::page_operations::__api_install_fd33eacf87();

    routes::access::previews::__api_install_7a9ea67528();

    routes::access::requests::__api_install_9eac981cd7();

    routes::access::requests::__api_install_eaa16a4cf5();

    routes::access::subject::__api_install_18e0c5f57f();

    routes::access::subject::__api_install_6ea4d24b43();

    routes::access::subject::__api_install_8dfafb0b48();

    routes::access::subject::__api_install_873b95e8e2();

    routes::access::subject::__api_install_a0c3c93238();

    routes::access::__api_install_8101ceef54();

    routes::access::__api_install_504712b347();

    routes::access::__api_install_2c7309614f();

    routes::access::__api_install_6c1917a46a();

    routes::access::__api_install_da8675c2ca();

    routes::access::__api_install_e93405a280();

    routes::chats::duplicate::__api_install_fce0a68441();

    routes::chats::duplicate::__api_install_b088dc274d();

    routes::chats::read_operation::__api_install_042efd7a1c();

    routes::chats::read_operation::__api_install_632caf7569();

    routes::chats::__api_install_9015882134();

    routes::chats::__api_install_2ec54615a9();

    routes::chats::__api_install_6acffcfdba();

    routes::chats::__api_install_fd7285312b();

    routes::chats::__api_install_9f835fb32e();

    routes::chats::__api_install_fde78fd2f0();

    routes::chats::__api_install_a49a3349ce();

    routes::everything::library::__api_install_874c2d3b53();

    routes::everything::library::__api_install_3582a8c8ce();

    routes::everything::__api_install_c47513c681();

    routes::nodes::content_sources::__api_install_8d4168e970();

    routes::nodes::content_sources::__api_install_d562690967();

    routes::nodes::make::__api_install_80201844f3();

    routes::nodes::make::__api_install_b69b8a0c63();

    routes::nodes::moves::thread::operation::__api_install_df48d8cc75();

    routes::nodes::moves::thread::operation::__api_install_01f987e298();

    routes::nodes::moves::thread::operation::__api_install_8039c90d48();

    routes::nodes::moves::thread::operation::__api_install_8ce72302f8();

    routes::nodes::moves::thread::writing::__api_install_3d9ed592e3();

    routes::nodes::moves::thread::__api_install_aae135d331();

    routes::nodes::moves::thread::__api_install_08cc559e9a();

    routes::nodes::moves::thread::__api_install_e3bfe7ff85();

    routes::nodes::moves::thread::__api_install_12543c587f();

    routes::nodes::moves::__api_install_ecca30487d();

    routes::nodes::moves::__api_install_ea3740bdc8();

    routes::nodes::projection::__api_install_7323c50a9b();

    routes::nodes::tree::__api_install_89b9752c11();

    routes::nodes::tree::__api_install_610ad28454();

    routes::nodes::tree::__api_install_0890f40f66();

    routes::nodes::tree::__api_install_a30b746301();

    routes::nodes::tree::__api_install_e99356e299();

    routes::nodes::tree_work::__api_install_7100f6658b();

    routes::nodes::view::__api_install_d95d290b5b();

    routes::nodes::view::__api_install_d13249f940();

    routes::nodes::view::__api_install_bc5d22e369();

    routes::nodes::view::__api_install_0e20fd9794();

    routes::nodes::view::__api_install_c92383aa47();

    routes::nodes::view::__api_install_d00ac38ead();

    routes::nodes::view::__api_install_9f5afc18da();

    routes::nodes::view::__api_install_04c4dded17();

    routes::nodes::view::__api_install_fd6dc0f5ec();

    routes::nodes::view::__api_install_d2452234dd();

    routes::nodes::view::__api_install_2d073aa738();

    routes::nodes::view::__api_install_ccc8c65fdf();

    routes::nodes::view::__api_install_800650d18e();

    routes::nodes::view::__api_install_c08cec6978();

    routes::nodes::view::__api_install_b615bcd360();

    routes::nodes::view::__api_install_88b5309e9a();

    routes::nodes::view::__api_install_6cc7a07912();

    routes::nodes::__api_install_6a67041e9d();

    routes::nodes::__api_install_323e7e7445();

    routes::nodes::__api_install_5bb02f8208();

    routes::nodes::__api_install_8b7af4dd87();

    routes::nodes::__api_install_ad86b076cb();

    routes::nodes::__api_install_4e4c0b11bd();

    routes::nodes::__api_install_ab7c5eef30();

    routes::nodes::__api_install_82375a5d88();

    routes::nodes::__api_install_d22b9e8a4f();

    routes::nodes::__api_install_d3e68dd0c4();

    routes::nodes::__api_install_fad93929f7();

    routes::nodes::__api_install_868fc59953();

    routes::nodes::tree_docs::__api_install_f52fcb6b67();

    routes::notifications::grouping::__api_install_76181556db();

    routes::notifications::inbox::__api_install_66aa9c259f();

    routes::notifications::inbox::__api_install_b96d6186ae();

    routes::notifications::inbox::__api_install_5ec55184a8();

    routes::notifications::inbox::__api_install_6023449a2d();

    routes::notifications::inbox::__api_install_a4b40e5758();

    routes::notifications::person::__api_install_9bbe43b245();

    routes::notifications::person::__api_install_80eea56439();

    routes::notifications::person::__api_install_7c8bf0625f();

    routes::notifications::person::__api_install_012d881745();

    routes::notifications::person::__api_install_013a538b95();

    routes::notifications::person::__api_install_9c90bbf056();

    routes::notifications::person::__api_install_9cdcfdd495();

    routes::notifications::person::__api_install_d216f11fcf();

    routes::notifications::person::__api_install_ef7d228569();

    routes::notifications::person::__api_install_bc3f2eea2c();

    routes::notifications::person::__api_install_981c9758e6();

    routes::notifications::__api_install_63f7322eaf();

    routes::notifications::__api_install_4e529c083e();

    routes::search::__api_install_737f400002();

    routes::trace::__api_install_5411811a96();

    routes::trace::__api_install_4d6aa3f24b();

    routes::trace::__api_install_7aeb1c6360();

    routes::transfer::__api_install_26428bb37a();
}
