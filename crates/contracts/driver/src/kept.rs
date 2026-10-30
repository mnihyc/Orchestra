//! Owned images that keep one environment's installed files across unavoidable same-image replacements.

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One owned local image, independent of the unit that originally recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeptImage {
    /// The system whose node may sweep this image.
    pub system: String,
    /// The environment whose files it holds.
    pub environment: Id,
    /// The generation that owns its name.
    pub generation: i64,
    /// The durable job that recorded this exact snapshot; failed attempts may reuse a generation number.
    pub capture: Id,
    /// The driver's local name, used only after verifying its ownership labels.
    pub reference: String,
    /// The size of owned layers, excluding the shared base unless the image was flattened.
    pub bytes: i64,
    /// The whole image's measured size, including shared base layers needed by an archive transfer.
    #[serde(default)]
    pub total_bytes: i64,
    /// Own committed layers since the last flattening.
    pub layers: u32,
}

/// A before-change estimate; running work may still change the writable layer afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeepEstimate {
    /// The unit's writable layer, or the whole root file system when flattening is due.
    pub bytes: i64,
    /// Estimated local recording time, from the measured one-second overhead plus thirty seconds/GiB.
    pub seconds: i64,
    /// Ten own committed layers require a flattening instead of another incremental layer.
    pub flatten: bool,
}

/// A generation which current units, archived environments or unfinished jobs still own on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeptOwner {
    pub environment: Id,
    pub generation: i64,
    /// The exact snapshot; absent only for an older record whose capture was not recorded.
    pub capture: Option<Id>,
}
