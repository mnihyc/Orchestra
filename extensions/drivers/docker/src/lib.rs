//! Docker Engine implementation of the runtime driver contract.
//!
//! [`DockerDriver`] makes the units that `orch-node` asks for on a host's Docker daemon, for both pools of runtimes:
//! a container for each environment and each provider's runtime, with its file system from a registry reference, a
//! Dockerfile or an image archive; its workspace, home and state in named volumes that survive stop, recreate and
//! update; mounts as bind mounts and volumes, read-only, shared, or as a private view; the connector from the
//! tool cache as its main process, started with the launch inputs of the request; networks on which a unit reaches
//! the gateway and nothing else of Orch's; limits the daemon enforces; capability patches merged into the
//! container's options; and labels by which the system finds its units again and never touches anybody else's.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`driver`] | The driver and the operations of the contract |
//! | [`config`] | The driver's settings |
//! | [`client`] | The connection to the daemon and what it was found to be |
//! | [`descriptor`] | The declaration, with the schemas of both pools |
//! | [`spec`] | The template spec as the driver reads it |
//! | [`units`] | Containers, their labels and their lifecycle |
//! | [`volumes`] | Volumes on the host's storages |
//! | [`mounts`], [`overlay`] | Mounts, and private views laid under a layer |
//! | [`images`], [`build`], [`archive`] | Images from a reference, a Dockerfile or an archive |
//! | [`imports`] | Proposing a spec from a Dockerfile, an image or an archive |
//! | [`networks`] | The networks units are put on |
//! | [`egress`] | The host's proxy of a restricted unit: what it admits, its resolver and its program |
//! | [`restricted`] | A restricted unit's own network and its proxy's container, made, started, stopped and removed with it |
//! | [`capabilities`] | Capability patches |
//! | [`guest`] | What a unit under a runtime with a kernel of its own needs: a resolver file, Docker inside's data place |
//! | [`runtimes`] | The daemon's runtimes as declared classes: boundary, offers, limits |
//! | [`state_moves`] | A stopped unit's files below one directory of its state, moved into another unit |
//! | [`stats`] | Resource use of a unit |
//! | [`progress`] | Progress of long operations |

#![forbid(unsafe_code)]

pub mod archive;
pub mod build;
pub mod capabilities;
pub mod client;
pub mod config;
pub mod descriptor;
pub mod driver;
pub mod egress;
pub mod guest;
pub mod images;
pub mod imports;
pub mod kept_images;
pub mod live_capabilities;
pub mod live_mounts;
pub mod live_resources;
pub mod mounts;
pub mod networks;
pub mod overlay;
pub mod progress;
pub mod resources;
pub mod restricted;
pub mod runtimes;
pub mod spec;
pub mod state_moves;
pub mod stats;
pub mod units;
pub mod volume_moves;
pub mod volumes;

pub use archive::BlobSource;
pub use config::DockerConfig;
pub use driver::DockerDriver;
