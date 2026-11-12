//! Kubernetes implementation of the runtime driver contract.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod admission;
#[cfg(target_os = "linux")]
mod authenticated_control;
mod capacity;
#[cfg(target_os = "linux")]
mod claim_service;
#[cfg(target_os = "linux")]
mod claims;
#[cfg(target_os = "linux")]
mod class_limits;
#[cfg(target_os = "linux")]
mod client;
pub mod config;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
pub use driver::KubernetesDriver;
pub use schema::configuration_schema;
#[cfg(target_os = "linux")]
mod control_protocol;
#[cfg(target_os = "linux")]
mod encrypted_transport;
#[cfg(target_os = "linux")]
mod guard_dispatch;
#[cfg(target_os = "linux")]
mod guard_link;
#[cfg(target_os = "linux")]
mod guard_stop;
#[cfg(target_os = "linux")]
mod helper;
#[cfg(target_os = "linux")]
pub use helper::main as helper_main;
#[cfg(target_os = "linux")]
mod launch;
#[cfg(target_os = "linux")]
mod launch_store;
#[cfg(target_os = "linux")]
mod locks;
#[cfg(target_os = "linux")]
mod logs;
#[cfg(target_os = "linux")]
mod names;
#[cfg(target_os = "linux")]
mod ownership;
#[cfg(target_os = "linux")]
mod peer;
#[cfg(target_os = "linux")]
mod platform;
#[cfg(target_os = "linux")]
mod pod;
#[cfg(target_os = "linux")]
mod pod_identity;
#[cfg(target_os = "linux")]
mod pod_read;
#[cfg(target_os = "linux")]
mod private_environment;
#[cfg(target_os = "linux")]
mod process_scope;
#[cfg(target_os = "linux")]
mod receipts;
#[cfg(target_os = "linux")]
mod relay;
#[cfg(target_os = "linux")]
mod run_files;
mod schema;
#[cfg(target_os = "linux")]
mod stage_exchange;
#[cfg(target_os = "linux")]
mod stage_protocol;
#[cfg(target_os = "linux")]
mod stage_worker;
#[cfg(target_os = "linux")]
mod staging;
#[cfg(target_os = "linux")]
mod subreaper;
#[cfg(target_os = "linux")]
mod supervisor_protocol;
#[cfg(target_os = "linux")]
mod supervisor_state;
#[cfg(target_os = "linux")]
mod template_schema;
#[cfg(target_os = "linux")]
mod unit_create;
#[cfg(target_os = "linux")]
mod verify_admission;
#[cfg(target_os = "linux")]
mod verify_claim;

#[cfg(all(test, feature = "k3d-test"))]
#[cfg(target_os = "linux")]
mod k3d_test;
