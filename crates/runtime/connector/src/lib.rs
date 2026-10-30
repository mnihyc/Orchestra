//! Managed and connected role host for agent, provider and environment services.
//!
//! The connector is the program that runs on a machine so that Orch can use it: on a person's computer,
//! where the person enrolled it, and in a runtime the system made. It holds one session to the gateway
//! and answers the commands it is sent, in the roles it plays. This crate is the part of it that is no
//! program: the pieces a program composes.
//!
//! * Who it is and how it joins a system: [`enrollment`], [`session`], [`policy`].
//! * What it does: [`dispatch`] carries the system's commands to the **roles** ([`roles`]) that own
//!   them; the agent role ([`roles::agent`]) hosts harnesses through adapters ([`harnesses`],
//!   [`reconciliation`]); the provider role, the console, the builds, the update and the service
//!   integrations are added by their own parts through the same two seams: a [`roles::RoleHandler`]
//!   for a role, and a [`loopback::Endpoint`] for an address a harness calls.
//! * What it does for an environment: the environment role ([`roles::environment`]) carries each
//!   command about an environment to the environment host with the owner's limits applied first and
//!   publishes what the host reports; a person's shell is taken up through [`shells`]; the executors
//!   and tool servers of an environment are reached through [`tool_tunnels`] (the end where they run)
//!   and [`tool_endpoints`] (the end where the harness runs); the tool servers a harness's own
//!   configuration names are found, imported and forgotten by [`mcp_import`].
//! * The machinery around them: [`events`] (what an adapter reports, as events of the link),
//!   [`content`] and [`https`] (what moves beside the session), [`ledger`] (what is hosted, on disk),
//!   [`facts`] (what the machine reports of itself), [`loopback`] (the listener for harnesses),
//!   [`tunnels`] (the tunnels other machines open to this one, handed to the role that serves them).
//! * The provider role ([`roles::provider`]) keeps a provider's key ([`credentials`]), holds the leases
//!   ([`leases`]) under which it may be used, and serves the model calls of harnesses beside it and
//!   through tunnels ([`forwarder`], [`upstream`]), counting what they use ([`usage`]).
//! * [`connector`] starts and stops all of it.

#![forbid(unsafe_code)]

pub mod after_up;
pub mod automatic;
pub mod background;
pub mod browser_tunnel;
pub mod builds;
pub mod connector;
pub mod console;
pub mod content;
pub mod credentials;
pub mod dispatch;
pub mod display_tunnel;
pub mod doctor;
pub mod enrollment;
pub mod events;
pub mod facts;
pub mod forwarder;
pub mod harnesses;
pub mod host_ports;
pub mod host_publish;
pub mod https;
pub mod installation;
pub mod key_facts;
pub mod leases;
pub mod ledger;
pub mod lifecycle;
pub mod local_ports;
pub mod local_socket;
pub mod loopback;
pub mod mcp_import;
pub mod orch_tools;
pub mod policy;
pub mod port_forward;
pub mod ports_carrier;
pub mod programs;
pub mod reconciliation;
pub mod role_changes;
pub mod roles;
pub mod service;
pub mod session;
pub mod shells;
pub mod skills;
pub mod sources;
pub mod storages;
pub mod tool_endpoints;
pub mod tool_tunnels;
pub mod tunnels;
pub mod uninstall;
pub mod update;
pub mod upstream;
pub mod usage;
