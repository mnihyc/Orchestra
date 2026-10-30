//! HTTP request and response contracts, with route metadata.
//!
//! Types are written as the API has them on the wire; [`routes::all`] lists
//! every request with who may make it.

#![forbid(unsafe_code)]

pub mod access;
pub mod admin;
pub mod approvals;
pub mod auth;
pub mod background;
pub mod binding;
pub mod browsers;
pub mod calls;
pub mod chats;
pub mod common;
pub mod context;
pub mod directory;
pub mod displays;
pub mod document_tree;
pub mod documents;
pub mod downloads;
pub mod environment;
pub mod everything;
pub mod exports;
pub mod first_steps;
pub mod forks;
pub mod goals;
pub mod imports;
pub mod input_schedules;
pub mod instructions;
pub mod library;
pub mod links;
pub mod loss;
pub mod machine_programs;
pub mod messages;
pub mod model_inputs;
pub mod mounts;
pub mod native;
pub mod nodes;
pub mod notifications;
pub mod openapi;
pub mod ownership;
pub mod package;
pub mod performance;
pub mod personal;
pub mod ports;
pub mod processes;
pub mod program_admin;
pub mod programs;
pub mod resource_facts;
pub mod resources;
pub mod routes;
pub mod search;
pub mod settings;
pub mod share;
pub mod skills;
pub mod source;
pub mod storage;
pub mod terminals;
pub mod thread_move;
pub mod threads;
pub mod topics;
pub mod trust;
pub mod usage;
pub mod users;
pub mod workspace;
