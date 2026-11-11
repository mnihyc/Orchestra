//! Real metadata invalidations after a completed singleton snapshot, before final publication.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use super::*;

pub use orch_api_base::routes::resources::projection::single::tests::Change;

pub use orch_api_base::routes::resources::projection::single::tests::Boundary;

pub use orch_api_base::routes::resources::projection::single::tests::after_snapshot;

pub use orch_api_base::routes::resources::projection::single::tests::control;
#[cfg(any(test, feature = "test-support"))]
async fn document_cut_rejects_a_changed_hierarchy_after_snapshot_commit() {
    control(K::Document, Change::Hierarchy).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_document_cut_rejects_a_changed_hierarchy_after_snapshot_commit()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::document_cut_rejects_a_changed_hierarchy_after_snapshot_commit())
}

#[cfg(any(test, feature = "test-support"))]
async fn folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit() {
    control(K::DocumentContainer, Change::Membership).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn document_metadata_finisher_keeps_read_after_completed_manage_is_removed() {
    control(K::Document, Change::KeepRead).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_document_metadata_finisher_keeps_read_after_completed_manage_is_removed()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::document_metadata_finisher_keeps_read_after_completed_manage_is_removed(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn folder_metadata_finisher_keeps_read_after_completed_manage_is_removed() {
    control(K::DocumentContainer, Change::KeepRead).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_folder_metadata_finisher_keeps_read_after_completed_manage_is_removed()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::folder_metadata_finisher_keeps_read_after_completed_manage_is_removed(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn document_metadata_finisher_withholds_read_lost_after_commit() {
    control(K::Document, Change::KeepManage).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_document_metadata_finisher_withholds_read_lost_after_commit()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::document_metadata_finisher_withholds_read_lost_after_commit())
}

#[cfg(any(test, feature = "test-support"))]
async fn folder_metadata_finisher_withholds_read_lost_after_commit() {
    control(K::DocumentContainer, Change::KeepManage).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_folder_metadata_finisher_withholds_read_lost_after_commit()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::folder_metadata_finisher_withholds_read_lost_after_commit())
}

#[cfg(any(test, feature = "test-support"))]
async fn a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt() {
    control(K::Document, Change::Expire).await;
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt(),
    )
}
