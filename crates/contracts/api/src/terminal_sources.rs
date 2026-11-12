//! Actual session governors; actions and business families remain in terminals.

use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetUsage, TargetResolver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The live environment on the agent terminal's stored thread.
    AgentEnvironment,
    /// The actual environment on a stored shell session.
    SessionEnvironment,
    /// The actual thread only when its audit authority is the selected recording door.
    RecordingThread,
    /// The existing environment governing creation and every owned shell given up there.
    OpeningEnvironment,
}

const AGENT: &[Binding<Source>] = &[Binding {
    source: Source::AgentEnvironment,
    target: TargetResolver::Resolved("thread_environment"),
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
}];

pub const AGENT_VIEW: Declaration<Source> = Declaration {
    operation: "AgentTerminalTicketCreate.view",
    bindings: AGENT,
};

pub const AGENT_CONTROL: Declaration<Source> = Declaration {
    operation: "AgentTerminalTicketCreate.control",
    bindings: AGENT,
};

pub const RECORDING: Declaration<Source> = Declaration {
    operation: "TerminalRecordingGet",
    bindings: &[
        Binding {
            source: Source::SessionEnvironment,
            target: TargetResolver::Resolved("terminal_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::RecordingThread,
            target: TargetResolver::ResolvedCollection("recording_audit_thread"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
    ],
};

pub const CLOSE: Declaration<Source> = Declaration {
    operation: "TerminalClose",
    bindings: &[
        Binding {
            source: Source::SessionEnvironment,
            target: TargetResolver::Resolved("terminal_environment"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: Source::SessionEnvironment,
            target: TargetResolver::Resolved("terminal_environment"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
    ],
};

pub const CREATE: Declaration<Source> = Declaration {
    operation: "EnvironmentTerminalCreate",
    bindings: &[Binding {
        source: Source::OpeningEnvironment,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
