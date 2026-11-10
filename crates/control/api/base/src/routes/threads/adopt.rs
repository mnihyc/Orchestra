//! A session that a person began in a harness by hand, taken in as a thread of a topic: found by asking the harness, put in the folder of theirs that holds the directory it ran in, and checked as any thread is for what the caller may use.
//!
//! The engine does the work ([`orch_engine::adoption`]); this module reads the request, and turns
//! what went wrong into the problem a client acts on: a session the harness does not hold is not
//! there, one that a thread took in already is a conflict that names the thread, and a person with no
//! environment on that machine whose folder holds the session's directory is asked to register
//! the folder first. Its preview answers what adopting would do (where the thread would stand, or the
//! folder to register first; the model it would run on; how much of the session's history comes)
//! before anything does, and writes nothing.

#[path = "adopt/operation.rs"]
pub mod operation;
