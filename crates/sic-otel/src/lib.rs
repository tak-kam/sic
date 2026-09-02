//! Converting an execution journal into OpenTelemetry.
//!
//! The journal is the canonical record of a run; OTLP is a view of it. The
//! arrow points one way, and none of the OTel vocabulary reaches back into the
//! event model: if the standard changes, this crate changes.
//!
//! It converts and does not send. Sending telemetry is an external effect, and
//! an external effect is a capability - a VM that could quietly post spans
//! somewhere would be the exfiltration path the journal was careful not to
//! build. So this produces a document, and getting it to a collector is
//! somebody else's job.
//!
//! Being a pure function of the journal also means it can run long after the
//! run finished, on a machine that never saw it.

mod json;
mod metrics;
mod traces;

pub use metrics::metrics;
pub use traces::traces;

/// What the exported telemetry says it came from.
#[derive(Debug, Clone)]
pub struct Resource {
    pub service_name: String,
    pub service_version: String,
}

impl Default for Resource {
    fn default() -> Self {
        Self {
            service_name: "sic".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// The scope every span and metric here belongs to.
pub const SCOPE_NAME: &str = "sic";

/// Attributes in the language's own namespace, per section 24 of the
/// specification.
pub mod attr {
    pub const RUN_ID: &str = "sic.run.id";
    pub const TASK_ID: &str = "sic.task.id";
    pub const WORKFLOW: &str = "sic.workflow.name";
    pub const CAPABILITY: &str = "sic.capability.name";
    pub const ATTEMPT: &str = "sic.capability.attempt";
    pub const FUNCTION: &str = "sic.function.name";
    pub const ARGS_DIGEST: &str = "sic.args.digest";
    /// Which bytecode the run was of. On the root span, because it is true of
    /// the whole run, and because somebody reading a trace they did not produce
    /// has no other way to ask what program it is about.
    pub const PROGRAM_DIGEST: &str = "sic.program.digest";
    pub const RESULT_DIGEST: &str = "sic.result.digest";
    pub const BUDGET_REMAINING: &str = "sic.budget.remaining";

    /// The GenAI conventions, for a model call.
    pub const GEN_AI_SYSTEM: &str = "gen_ai.system";
    pub const GEN_AI_OPERATION: &str = "gen_ai.operation.name";
}
