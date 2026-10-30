//! Kubernetes implementation of the runtime driver contract.

#![forbid(unsafe_code)]

mod admission;
mod authenticated_control;
mod capacity;
mod claim_service;
mod claims;
mod class_limits;
mod client;
pub mod config;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
pub use driver::KubernetesDriver;
pub use schema::configuration_schema;
mod control_protocol;
mod encrypted_transport;
mod guard_dispatch;
#[cfg(target_os = "linux")]
mod guard_link;
mod guard_stop;
#[cfg(target_os = "linux")]
mod helper;
#[cfg(target_os = "linux")]
pub use helper::main as helper_main;
mod launch;
#[cfg(target_os = "linux")]
mod launch_store;
mod locks;
mod logs;
mod names;
mod ownership;
mod peer;
mod platform;
mod pod;
mod pod_identity;
mod pod_read;
mod private_environment;
mod process_scope;
mod receipts;
#[cfg(target_os = "linux")]
mod relay;
mod run_files;
mod schema;
#[cfg(target_os = "linux")]
mod stage_exchange;
mod stage_protocol;
#[cfg(target_os = "linux")]
mod stage_worker;
mod staging;
mod subreaper;
#[cfg(target_os = "linux")]
mod supervisor_protocol;
mod supervisor_state;
mod template_schema;
mod unit_create;
mod verify_admission;
mod verify_claim;

#[cfg(all(test, feature = "k3d-test"))]
mod k3d_test;
