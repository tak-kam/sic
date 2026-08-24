//! What the agent answering a model call may do.
//!
//! The rule is one sentence: the agent's authority is the program's manifest,
//! and nothing more. This works out, for each grant, how that grant reaches the
//! agent - or refuses the run, because a manifest that cannot be enforced is
//! worse than none once `sic plan` has printed it.
//!
//! See `docs/design/authority.md`.

use std::fmt;

use sic_core::CapGrant;

/// How one grant reaches the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// The grant that summoned the agent. It is the authority being exercised
    /// by asking, not authority the agent has while answering.
    Summons,
    /// The agent's own permission system can enforce this grant's constraint,
    /// so it becomes a rule in that system.
    Translated(Vec<Rule>),
    /// It cannot, so the agent's own tool is denied and the capability is
    /// offered back through the broker instead - where it is performed by the
    /// same code, against the same constraint, into the same journal.
    Routed(&'static str),
    /// Neither, so nothing can enforce it and the run does not start.
    Unenforceable(&'static str),
}

/// One rule in the agent's permission configuration.
///
/// A tool, and what it is bounded to when the tool takes a bound. Rendering is
/// one `Display` impl because the vocabulary belongs to the agent and moves
/// with its version, the way what a pane prints does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub tool: &'static str,
    pub scope: Option<String>,
    /// Appended to the tool name rather than parenthesised, which is how a
    /// server's tools are named: `mcp__sic__process_exec`.
    pub suffix: Option<String>,
}

/// How the agent names a tool this broker offers it.
pub const TOOL_PREFIX: &str = "mcp__sic__";

impl Rule {
    fn scoped(tool: &'static str, scope: &str) -> Rule {
        Rule {
            tool,
            scope: Some(scope.to_string()),
            suffix: None,
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tool)?;
        if let Some(suffix) = &self.suffix {
            f.write_str(suffix)?;
        }
        match &self.scope {
            Some(scope) => write!(f, "({scope})"),
            None => Ok(()),
        }
    }
}

/// Tools that reach the network.
///
/// Denied by rule as well as by the hook, because a deny rule applies across
/// every settings scope and cannot be overridden by an allow rule anywhere -
/// the one part of this configuration that is not merely additive. The hook is
/// what actually holds (§6); this is what holds if the hook is not reached.
pub const NETWORK_TOOLS: &[&str] = &["WebFetch", "WebSearch"];

/// What the agent may do, worked out from the program's manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authority {
    /// Rules the agent's own permission system will enforce.
    pub allowed: Vec<Rule>,
    /// Capabilities the agent must reach through the broker instead, named as
    /// the manifest names them.
    pub routed: Vec<String>,
}

impl Authority {
    /// The names of the tools this manifest accounts for.
    ///
    /// The hook refuses everything else, so this is the agent's whole tool
    /// surface - which is why it is a list of names and not of rules: a name is
    /// something the hook can check without re-implementing the path matching
    /// that belongs to the agent. Deny by name here, scope by rule there.
    pub fn tools(&self) -> Vec<String> {
        self.allowed
            .iter()
            .map(|rule| match &rule.suffix {
                Some(suffix) => format!("{}{suffix}", rule.tool),
                None => rule.tool.to_string(),
            })
            .collect()
    }

    /// The arguments that put this in front of the agent.
    ///
    /// `dontAsk` is the mode that makes an allowlist mean what it says: a tool
    /// that is named nowhere is denied without prompting. Any mode that can
    /// prompt would hang, because the pane has nobody watching it - so this is
    /// not a preference, it is the only mode a detached agent can run in.
    pub fn arguments(&self) -> Vec<String> {
        let mut out = vec!["--permission-mode".into(), "dontAsk".into()];
        if !self.allowed.is_empty() {
            out.push("--allowedTools".into());
            out.extend(self.allowed.iter().map(|r| r.to_string()));
        }
        out.push("--disallowedTools".into());
        out.extend(NETWORK_TOOLS.iter().map(|t| (*t).to_string()));
        out
    }
}

/// Why a run must not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub grant: String,
    pub why: String,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` cannot be enforced against the agent: {}",
            self.grant, self.why
        )
    }
}

/// Works out what the agent may do, or refuses the run.
///
/// Called before anything runs. Everything checkable before a run is checked
/// before it, and this is checkable: it reads the manifest and nothing else.
pub fn authority_of(manifest: &[CapGrant]) -> Result<Authority, Refused> {
    let mut authority = Authority::default();
    for grant in manifest {
        match reach_of(grant) {
            Reach::Summons => {}
            Reach::Translated(rules) => authority.allowed.extend(rules),
            // The agent's own tool stays denied; the capability arrives at
            // the broker instead, where it is authorized against this same
            // manifest and performed by the same code.
            Reach::Unenforceable(why) => {
                return Err(Refused {
                    grant: describe(grant),
                    why: why.to_string(),
                });
            }
            Reach::Routed(_) => {
                authority.routed.push(grant.name.clone());
                authority.allowed.push(Rule {
                    tool: TOOL_PREFIX,
                    scope: None,
                    suffix: Some(grant.name.replace('.', "_")),
                });
            }
        }
    }
    Ok(authority)
}

/// How one grant reaches the agent.
pub fn reach_of(grant: &CapGrant) -> Reach {
    match grant.name.as_str() {
        // The grant being exercised. Answering it is what the agent is for.
        "llm.invoke" => Reach::Summons,

        // A path scope is a thing a permission system can hold. `fs.read`
        // grants exactly one path - the broker refuses anything else - so the
        // rule is that path and not a prefix of it.
        "fs.read" => Reach::Translated(vec![Rule::scoped("Read", &grant.constraint)]),
        // Writing a whole file and editing part of one are the same authority
        // here, because `fs.write` replaces the file either way.
        "fs.write" => Reach::Translated(vec![
            Rule::scoped("Write", &grant.constraint),
            Rule::scoped("Edit", &grant.constraint),
        ]),

        // A shell rule looks like it fits and does not. `process.exec` grants
        // one binary, at an absolute path, sometimes pinned by digest; a rule
        // on a shell command is a match on a string that can invoke anything
        // it likes - a pipe, a different binary on `PATH`, the same name
        // shadowed. Translating one into the other would widen the grant to
        // fit the configuration's vocabulary, which is the one thing a
        // translation must never do.
        "process.exec" | "process.capture" => {
            Reach::Routed("a shell command is not a binary, and a digest pin has no equivalent")
        }

        // Asking a person is not a tool the agent has, and it suspends the run
        // when the broker performs it. Only the broker can do either.
        "human.approve" | "human.choose" => Reach::Routed("only the broker can ask a person"),

        // A capability the compiler knows and this broker does not. Routing it
        // would offer the agent a tool nothing can perform, and translating it
        // would need a constraint nobody here can read, so it is the case §5 is
        // for: the run does not start.
        _ => Reach::Unenforceable(
            "this broker cannot perform it, so it can be neither translated nor routed",
        ),
    }
}

/// A grant as a person reads it in a refusal.
fn describe(grant: &CapGrant) -> String {
    match grant.constraint.is_empty() {
        true => grant.name.clone(),
        false => format!("{} {:?}", grant.name, grant.constraint),
    }
}
