//! Exact native byte offsets survive lost replies without repeating a program input.
use bytes::Bytes;
use orch_driver_contract::{
    driver::DriverError,
    work::{WorkOutputStream, WorkProcess},
};
use orch_link::program_work::Answer;
use orch_link_contract::program_work::Reply;
use orch_types::Hash;
use std::sync::Arc;

struct Input {
    offset: u64,
    last: Option<(u64, Hash, u64)>,
    uncertain: bool,
}
struct Pending {
    offset: u64,
    stream: WorkOutputStream,
    bytes: Bytes,
}
struct Output {
    offset: u64,
    pending: Option<Pending>,
    remaining: Option<(WorkOutputStream, Bytes)>,
    ended: bool,
}
pub(super) struct Live {
    pub(super) process: Arc<dyn WorkProcess>,
    pub(super) options: Hash,
    input: tokio::sync::Mutex<Input>,
    output: tokio::sync::Mutex<Output>,
}

impl Live {
    pub(super) fn new(process: Arc<dyn WorkProcess>, options: Hash) -> Self {
        Self {
            process,
            options,
            input: tokio::sync::Mutex::new(Input {
                offset: 0,
                last: None,
                uncertain: false,
            }),
            output: tokio::sync::Mutex::new(Output {
                offset: 0,
                pending: None,
                remaining: None,
                ended: false,
            }),
        }
    }

    pub(super) async fn write(&self, offset: u64, bytes: &[u8]) -> Result<Reply, DriverError> {
        let mut input = self.input.lock().await;
        let hash = Hash::of(bytes);
        if let Some((old, kept, next)) = input.last
            && offset == old
            && hash == kept
        {
            return Ok(Reply::Written { next });
        }
        if offset != input.offset || input.uncertain {
            return Err(DriverError::Unavailable(
                "the program input offset is uncertain; no bytes were sent again".into(),
            ));
        }
        let next =
            offset
                .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                    DriverError::InvalidSpec("program input length overflow".into())
                })?)
                .ok_or_else(|| DriverError::InvalidSpec("program input offset overflow".into()))?;
        input.uncertain = true;
        self.process.write(bytes).await?;
        input.offset = next;
        input.last = Some((offset, hash, next));
        input.uncertain = false;
        Ok(Reply::Written { next })
    }
    pub(super) async fn output(&self, offset: u64, limit: usize) -> Result<Answer, DriverError> {
        let mut output = self.output.lock().await;
        if let Some(pending) = &output.pending {
            let next = pending
                .offset
                .checked_add(u64::try_from(pending.bytes.len()).map_err(|_| {
                    DriverError::InvalidSpec("program output length overflow".into())
                })?)
                .ok_or_else(|| DriverError::InvalidSpec("program output offset overflow".into()))?;
            if offset == next {
                output.offset = next;
                output.pending = None;
            }
        }
        if offset != output.offset {
            return Err(DriverError::InvalidSpec(
                "the program output offset does not match its retained stream".into(),
            ));
        }
        while output.pending.is_none() && !output.ended {
            if let Some((stream, mut bytes)) = output.remaining.take() {
                let piece = bytes.split_to(bytes.len().min(limit.max(1)));
                if !bytes.is_empty() {
                    output.remaining = Some((stream, bytes));
                }
                output.pending = Some(Pending {
                    offset,
                    stream,
                    bytes: piece,
                });
                break;
            }
            match self.process.next_output().await? {
                Some(chunk) if !chunk.bytes.is_empty() => {
                    output.remaining = Some((chunk.stream, Bytes::from(chunk.bytes)))
                }
                Some(_) => {}
                None => output.ended = true,
            }
        }
        Ok(match &output.pending {
            Some(pending) => Answer {
                reply: Reply::Output {
                    offset: pending.offset,
                    stream: pending.stream,
                    bytes: u64::try_from(pending.bytes.len()).map_err(|_| {
                        DriverError::InvalidSpec("program output length overflow".into())
                    })?,
                },
                bytes: pending.bytes.to_vec(),
            },
            None => Answer {
                reply: Reply::End { offset },
                bytes: vec![],
            },
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a case fails by panicking")]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_driver_contract::work::WorkOutput;
    use std::sync::Mutex;
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct Native {
        written: Mutex<Vec<u8>>,
        writes: AtomicUsize,
        fail: AtomicBool,
        pulls: AtomicUsize,
        output: Mutex<VecDeque<WorkOutput>>,
    }
    #[async_trait]
    impl WorkProcess for Native {
        fn reference(&self) -> &str {
            "retained-native-process"
        }
        async fn start(&self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn write(&self, bytes: &[u8]) -> Result<(), DriverError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.written.lock().unwrap().extend(bytes);
            if self.fail.swap(false, Ordering::SeqCst) {
                Err(DriverError::Unavailable(
                    "the write acknowledgement was lost".into(),
                ))
            } else {
                Ok(())
            }
        }
        async fn close_input(&self) -> Result<(), DriverError> {
            Ok(())
        }
        async fn next_output(&self) -> Result<Option<WorkOutput>, DriverError> {
            self.pulls.fetch_add(1, Ordering::SeqCst);
            Ok(self.output.lock().unwrap().pop_front())
        }
        async fn wait(&self) -> Result<Option<i64>, DriverError> {
            Ok(Some(0))
        }
        async fn terminate(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }
    fn fixture(fail: bool) -> (Arc<Native>, Live) {
        let native = Arc::new(Native {
            written: Mutex::new(vec![]),
            writes: AtomicUsize::new(0),
            fail: AtomicBool::new(fail),
            pulls: AtomicUsize::new(0),
            output: Mutex::new(VecDeque::from([
                WorkOutput {
                    stream: WorkOutputStream::Stdout,
                    bytes: b"abcdef".to_vec(),
                },
                WorkOutput {
                    stream: WorkOutputStream::Stderr,
                    bytes: b"x".to_vec(),
                },
            ])),
        });
        let live = Live {
            process: native.clone(),
            options: Hash::of(b"frozen options"),
            input: tokio::sync::Mutex::new(Input {
                offset: 0,
                last: None,
                uncertain: false,
            }),
            output: tokio::sync::Mutex::new(Output {
                offset: 0,
                pending: None,
                remaining: None,
                ended: false,
            }),
        };
        (native, live)
    }
    #[tokio::test]
    async fn a_lost_native_input_acknowledgement_never_sends_the_same_model_request_twice() {
        let (native, live) = fixture(false);
        let bytes = b"one native model request\n";
        let first = live.write(0, bytes).await.unwrap();
        assert_eq!(live.write(0, bytes).await.unwrap(), first);
        assert_eq!(native.writes.load(Ordering::SeqCst), 1);
        assert_eq!(*native.written.lock().unwrap(), bytes);
        assert!(live.write(0, b"a changed request\n").await.is_err());
        assert_eq!(native.writes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn an_uncertain_partial_native_write_is_never_completed_by_repeating_its_bytes() {
        let (native, live) = fixture(true);
        assert!(live.write(0, b"possibly paid\n").await.is_err());
        assert!(live.write(0, b"possibly paid\n").await.is_err());
        assert_eq!(native.writes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn native_output_survives_a_lost_reply_and_crosses_the_current_frame_allowance() {
        let (native, live) = fixture(false);
        let first = live.output(0, 3).await.unwrap();
        assert_eq!(first.bytes, b"abc");
        assert_eq!(live.output(0, 3).await.unwrap().bytes, b"abc");
        assert_eq!(native.pulls.load(Ordering::SeqCst), 1);
        assert_eq!(live.output(3, 3).await.unwrap().bytes, b"def");
        assert_eq!(native.pulls.load(Ordering::SeqCst), 1);
        let stderr = live.output(6, 3).await.unwrap();
        assert_eq!(stderr.bytes, b"x");
        assert!(matches!(
            stderr.reply,
            Reply::Output {
                stream: WorkOutputStream::Stderr,
                ..
            }
        ));
        assert!(matches!(
            live.output(7, 3).await.unwrap().reply,
            Reply::End { offset: 7 }
        ));
    }
}
