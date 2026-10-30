//! Browser socket frames, pictures and the acknowledgement that keeps only the newest image.

use orch_api_contract::users::UserRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The browser socket's subprotocol.
pub const BROWSER_SUBPROTOCOL: &str = "orch.browser.v1";
/// Twelve bytes: frame number followed by four two-byte dimensions.
pub const PICTURE_HEADER_BYTES: usize = 12;

/// A browser's page target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Tab {
    /// Its protocol identifier.
    pub id: String,
    /// Its page title.
    pub title: String,
    /// Its current address, carried live and never retained.
    pub url: String,
    /// Whether a navigation is underway.
    pub loading: bool,
    /// Whether this is the browser's active page.
    pub active: bool,
    /// The page that opened it.
    pub opener: Option<String>,
}

/// The shared browser window size in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Viewport {
    /// Its width.
    pub width: u16,
    /// Its height.
    pub height: u16,
}

/// People who watch and who sent input in the last ten seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPeople {
    /// Those who have a socket open.
    pub watching: Vec<UserRef>,
    /// Those who sent input recently.
    pub driving: Vec<UserRef>,
}

/// PointerKind values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    /// Move the pointer.
    Move,
    /// Press a button.
    Down,
    /// Release a button.
    Up,
    /// Scroll the page.
    Wheel,
}

/// PointerButton values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    /// The primary button.
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
}

/// KeyKind values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    /// Press a key.
    Down,
    /// Release a key.
    Up,
}

/// BrowserHistoryTo values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserHistoryTo {
    /// Go back.
    Back,
    /// Go forward.
    Forward,
    /// Reload the page.
    Reload,
    /// Stop loading.
    Stop,
}

/// BrowserTabAction values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTabAction {
    /// Open a page.
    New,
    /// Close a page.
    Close,
    /// Activate a page.
    Activate,
}

/// BrowserDialogKind values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDialogKind {
    /// An alert.
    Alert,
    /// A confirmation.
    Confirm,
    /// A text prompt.
    Prompt,
    /// A question before leaving the page.
    #[serde(rename = "beforeunload")]
    BeforeUnload,
}

/// BrowserQualityWhy values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserQualityWhy {
    /// Adapted to a busy link.
    Link,
    /// Drawn sharp after the page stood still.
    Clear,
}

/// BrowserSocketError values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSocketError {
    /// This ticket carries no input.
    ViewOnly,
    /// That tab no longer exists.
    NoSuchTab,
    /// The input needs another permission.
    NotAllowed,
    /// The frame is invalid.
    Invalid,
}

/// BrowserClosedReason values on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserClosedReason {
    /// A person quit it.
    Quit,
    /// Its agent closed it.
    ClosedByAgent,
    /// It was idle too long.
    Idle,
    /// Its process ended.
    Exited,
    /// Its environment stopped.
    EnvironmentStopped,
    /// The connector no longer has it.
    LinkLost,
    /// Browsers were switched off.
    SwitchedOff,
    /// The viewer lost access.
    Revoked,
}

/// What a viewer asks; a server forwards input only on a control ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserClientFrame {
    /// Select the page and panel size.
    Watch {
        /// The page target.
        tab: String,
        /// Panel width.
        width: u16,
        /// Panel height.
        height: u16,
        /// Screen pixel density.
        dpr: f64,
    },
    /// Acknowledge a picture that was drawn.
    Ack {
        /// The picture number.
        n: u32,
    },
    /// Measure the line.
    Ping {
        /// The number the answer repeats.
        n: i64,
    },
    /// Move, click or scroll.
    Pointer {
        /// The page target.
        tab: String,
        /// The pointer act.
        kind: PointerKind,
        /// Horizontal CSS coordinate.
        x: f64,
        /// Vertical CSS coordinate.
        y: f64,
        /// The button.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<PointerButton>,
        /// The pressed button mask.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        buttons: Option<u32>,
        /// The click count.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clicks: Option<u32>,
        /// Horizontal scroll delta.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dx: Option<f64>,
        /// Vertical scroll delta.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dy: Option<f64>,
        /// The modifier mask.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "mod")]
        modifiers: Option<u32>,
    },
    /// Press or release a key.
    Key {
        /// The page target.
        tab: String,
        /// Press or release.
        kind: KeyKind,
        /// The key value.
        key: String,
        /// The physical key.
        code: String,
        /// The key text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// The modifier mask.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[serde(rename = "mod")]
        modifiers: Option<u32>,
        /// Whether held down.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<bool>,
        /// The keyboard location.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<u32>,
    },
    /// Insert input-method or pasted text.
    Text {
        /// The page target.
        tab: String,
        /// Text inserted as it arrived.
        text: String,
    },
    /// Go to an address.
    Navigate {
        /// The page target.
        tab: String,
        /// The address.
        url: String,
    },
    /// Use page history or loading controls.
    History {
        /// The page target.
        tab: String,
        /// The action.
        to: BrowserHistoryTo,
    },
    /// Open, close or activate a page.
    Tab {
        /// The action.
        #[serde(rename = "do")]
        action: BrowserTabAction,
        /// The existing page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<String>,
        /// The initial address.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Answer a page question.
    Dialog {
        /// The page target.
        tab: String,
        /// Whether accepted.
        accept: bool,
        /// The prompt answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Answer a file chooser with workspace files.
    Files {
        /// The page target.
        tab: String,
        /// Workspace paths, or null to cancel.
        paths: Option<Vec<String>>,
    },
    /// Resize the shared browser window.
    Viewport {
        /// CSS width.
        width: u16,
        /// CSS height.
        height: u16,
    },
    /// Read the selected text.
    Copy {
        /// The page target.
        tab: String,
    },
}

/// Live page metadata and control replies; no frame is retained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BrowserServerFrame {
    /// The browser socket is ready.
    Ready {
        /// Whether it accepts input.
        control: bool,
        /// The pages.
        tabs: Vec<Tab>,
        /// The shared window size.
        viewport: Viewport,
        /// The viewers and recent drivers.
        people: BrowserPeople,
    },
    /// The page list changed.
    Tabs {
        /// The pages.
        tabs: Vec<Tab>,
    },
    /// Viewers or drivers changed.
    People {
        /// The viewers.
        watching: Vec<UserRef>,
        /// Recent drivers.
        driving: Vec<UserRef>,
    },
    /// A watched page asks a driver.
    Dialog {
        /// The page target.
        tab: String,
        /// The question kind.
        kind: BrowserDialogKind,
        /// The page question.
        message: String,
        /// The initial prompt text.
        default: Option<String>,
    },
    /// A page question ended.
    DialogClosed {
        /// The page target.
        tab: String,
    },
    /// A watched page asks for a file.
    Chooser {
        /// The page target.
        tab: String,
        /// Whether it accepts several.
        multiple: bool,
    },
    /// The picture level changed.
    Quality {
        /// The level from zero through four.
        level: u16,
        /// Why it changed.
        why: BrowserQualityWhy,
    },
    /// The selected text.
    Copied {
        /// Text inserted as it arrived.
        text: String,
    },
    /// A frame could not be applied.
    Error {
        /// The error code.
        code: BrowserSocketError,
        /// What needs correcting.
        detail: String,
    },
    /// This socket ended.
    Closed {
        /// Why.
        reason: BrowserClosedReason,
    },
    /// The answer to a ping.
    Pong {
        /// The question number.
        n: i64,
    },
}

impl BrowserClientFrame {
    /// Whether this frame can reach a page only through a control ticket.
    #[must_use]
    pub const fn needs_control(&self) -> bool {
        !matches!(
            self,
            Self::Watch { .. } | Self::Ack { .. } | Self::Ping { .. }
        )
    }
}

/// The big-endian prefix of one JPEG picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureHeader {
    /// Its wrapping sequence number.
    pub n: u32,
    /// The page's CSS dimensions.
    pub viewport: Viewport,
    /// The JPEG's pixel dimensions.
    pub image: Viewport,
}

impl PictureHeader {
    /// Writes the twelve-byte prefix.
    #[must_use]
    pub fn encode(self) -> [u8; PICTURE_HEADER_BYTES] {
        let mut out = [0; PICTURE_HEADER_BYTES];
        out[..4].copy_from_slice(&self.n.to_be_bytes());
        for (at, n) in [
            (4, self.viewport.width),
            (6, self.viewport.height),
            (8, self.image.width),
            (10, self.image.height),
        ] {
            out[at..at + 2].copy_from_slice(&n.to_be_bytes());
        }
        out
    }

    /// Reads a picture prefix, refusing a truncated frame.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let b: &[u8; PICTURE_HEADER_BYTES] = bytes.get(..PICTURE_HEADER_BYTES)?.try_into().ok()?;
        Some(Self {
            n: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            viewport: Viewport {
                width: u16::from_be_bytes([b[4], b[5]]),
                height: u16::from_be_bytes([b[6], b[7]]),
            },
            image: Viewport {
                width: u16::from_be_bytes([b[8], b[9]]),
                height: u16::from_be_bytes([b[10], b[11]]),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_viewer_acknowledges_the_picture_it_drew() {
        let frame: BrowserClientFrame = serde_json::from_value(json!({"op":"ack","n":7})).unwrap();
        assert!(!frame.needs_control());
        assert!(
            serde_json::from_value::<BrowserClientFrame>(json!({"op":"ack","n":7,"key":"x"}))
                .is_err()
        );
    }

    #[test]
    fn a_picture_carries_css_coordinates_separately_from_image_pixels() {
        let header = PictureHeader {
            n: 0x12345678,
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
            image: Viewport {
                width: 640,
                height: 400,
            },
        };
        assert_eq!(PictureHeader::decode(&header.encode()), Some(header));
        assert_eq!(&header.encode()[..4], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(PictureHeader::decode(&[0; 11]), None);
    }
}
