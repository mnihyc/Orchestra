//! Opt-in native evidence from the production raw-frame observer; public fixtures require a separate review.
use orch_agent_contract::events::SessionEvent;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct NativeRecording {
    output: Mutex<File>,
    frames: AtomicUsize,
    manifest: PathBuf,
    case: String,
    version: String,
    image: String,
    feature_flags_off: bool,
}

impl NativeRecording {
    pub fn from_env(case: &str, version: &str, image: &str) -> Option<Self> {
        let base = PathBuf::from(std::env::var_os("ORCH_NATIVE_RECORDING_DIR")?);
        // Raw native payloads stay in ignored private artifacts, never beside public replay fixtures.
        assert!(
            base.components()
                .any(|part| part.as_os_str() == std::ffi::OsStr::new(".tmp")),
            "ORCH_NATIVE_RECORDING_DIR must be under a private .tmp directory"
        );
        assert!(
            !case.is_empty()
                && case
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
            "the native case needs a safe filename"
        );
        let directory = base.join("claude-code");
        std::fs::create_dir_all(&directory).expect("create private native recording directory");
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{case}.frames.jsonl")))
            .expect(
                "create a fresh native recording; choose a new output directory for another run",
            );
        Some(Self {
            output: Mutex::new(output),
            frames: AtomicUsize::new(0),
            manifest: directory.join(format!("{case}.json")),
            case: case.to_owned(),
            version: version.to_owned(),
            image: image.to_owned(),
            feature_flags_off: std::env::var("ORCH_CLAUDE_FEATURE_FLAGS_OFF").as_deref() == Ok("1"),
        })
    }

    pub fn observe(&self, event: &SessionEvent) {
        if let SessionEvent::RawFrame(frame) = event {
            let mut output = self.output.lock().expect("native recording writer");
            // Preserve direction and every decoded native member, including unrecognized fields.
            serde_json::to_writer(&mut *output, frame).expect("write the observed native frame");
            output
                .write_all(b"\n")
                .expect("finish the native JSON line");
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn finish(&self, passed: bool, informational: bool) {
        self.output
            .lock()
            .expect("native recording writer")
            .flush()
            .expect("flush observed native frames");
        let frames = self.frames.load(Ordering::Relaxed);
        let mut manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.manifest)
            .expect("create the native recording manifest");
        serde_json::to_writer_pretty(
            &mut manifest,
            &json!({
                "source": "production raw.frame observer of the isolated native program",
                "format": "direction and decoded native JSON; no payload reconstruction",
                "capture_window": "native conformance case event consumption",
                "case": self.case,
                "expected_version": self.version,
                "image": self.image,
                "feature_flags_off_requested": self.feature_flags_off,
                "case_passed": passed,
                "informational": informational,
                "raw_frame_count": frames,
            }),
        )
        .expect("write the native recording manifest");
        // The sandbox refusal intentionally offers no native session to record.
        if passed && !informational && self.case != "sandbox" {
            assert!(
                frames > 0,
                "a passing native recording must contain actual raw frames"
            );
        }
    }
}
