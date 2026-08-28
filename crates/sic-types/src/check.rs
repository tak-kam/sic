//! Type checking and name resolution.
//!
//! Functions are checked in declaration order. A function without a return type
//! annotation gets its type from the `return` statements in its body, so calling
//! it before it has been checked requires an annotation (E0306). That rule keeps
//! inference to a single forward pass with no constraint solving.

use std::collections::{HashMap, HashSet};

use sic_core::{AgentId, Answers, CapId, Diagnostic, FuncId, Label, LocalId, NodeId, Span, TypeId};
use sic_syntax::ast::*;

use crate::cap::{self, CapEntry};
use crate::ty::{ObjectId, TrustKind, Type, Types};

/// What a name in an expression refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Res {
    Local(LocalId),
    Fn(FuncId),
    /// A capability, resolved to its index in the module's manifest.
    Cap(CapId),
    /// A built-in function, which lowers to an instruction rather than a call.
    Builtin(Builtin),
    /// An agent, which lowers to a model call and a validation.
    Agent(AgentId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Len,
    /// `approve(question, value)`, which asks a person and fails if the answer
    /// is no.
    Approve,
    /// `choose(question, options)`, which asks a person which one.
    Choose,
    /// `from_json(text)`, whose result type comes from the annotation on the
    /// binding it initializes.
    FromJson,
    /// `contains(haystack, needle)`: whether the needle occurs anywhere.
    Contains,
    /// `starts_with(s, prefix)`: whether it occurs at the start.
    StartsWith,
}

#[derive(Debug, Clone)]
pub struct FnInfo {
    pub name: String,
    /// Parameter types, in order. Parameters occupy locals 0..params.len().
    pub params: Vec<TypeId>,
    pub ret: TypeId,
    /// Types of the named locals, indexed by `LocalId`.
    pub local_types: Vec<TypeId>,
    /// Index of this function in `Module::items`.
    pub item_index: usize,
    pub span: Span,
}

/// An agent: a model call and the shape its answer has to fit.
///
/// Nothing below this layer knows what an agent is. It lowers to a capability
/// call and a `from_json`, which is the whole of what the declaration buys: the
/// output type is written once, and the run fails at the model boundary rather
/// than wherever the malformed value is first used.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub input: TypeId,
    pub output: TypeId,
    /// How many model calls the agent may make in a whole run.
    pub budget: Option<u32>,
    /// The conversation this agent's calls belong to, when it keeps one, and
    /// `None` when every call starts a fresh one.
    ///
    /// A number rather than a flag because two agents that both remember must
    /// not end up talking into the same conversation: what identifies one is
    /// the agent and the task, and the task is the only half the broker can
    /// see for itself.
    pub conversation: Option<u32>,
    /// How many tools the agent may use at this call site in a whole run, and
    /// how long it has to produce one answer. Both are the broker's to enforce:
    /// only it sees the agent's tools, and only it has a clock.
    pub tools: Option<u32>,
    pub deadline_ms: Option<u32>,
    /// The manifest entry for `llm.invoke`.
    pub cap: CapId,
}

/// Everything the later phases need on top of the AST.
#[derive(Debug)]
pub struct Typed {
    pub types: Types,
    pub fns: Vec<FnInfo>,
    /// The type of every expression, keyed by its `NodeId`.
    pub node_types: HashMap<NodeId, TypeId>,
    /// What each name expression resolved to.
    pub res: HashMap<NodeId, Res>,
    /// The capabilities the module granted itself, in manifest order.
    pub caps: Vec<CapEntry>,
    /// The agents the module declares.
    pub agents: Vec<AgentInfo>,
    /// The `main` function, if the module has one.
    pub entry: Option<FuncId>,
}

impl Typed {
    pub fn type_of(&self, node: NodeId) -> TypeId {
        self.node_types.get(&node).copied().unwrap_or(Types::ERROR)
    }

    pub fn res_of(&self, node: NodeId) -> Option<Res> {
        self.res.get(&node).copied()
    }
}

/// Checks a module. Diagnostics are collected rather than raised.
pub fn check(module: &Module) -> (Typed, Vec<Diagnostic>) {
    let mut c = Checker::new();
    c.collect_types(module);
    c.collect_capabilities(module);
    c.collect_signatures(module);
    c.collect_agents(module);
    c.check_bodies(module);
    c.check_requires(module);
    c.finish()
}

/// A function signature while checking is still in progress.
struct FnState {
    name: String,
    params: Vec<TypeId>,
    /// `None` means "not annotated and not checked yet", which is what makes a
    /// forward reference an error instead of a guess.
    ret: Option<TypeId>,
    local_types: Vec<TypeId>,
    item_index: usize,
    span: Span,
}

struct Checker {
    types: Types,
    diags: Vec<Diagnostic>,
    fns: Vec<FnState>,
    fn_ids: HashMap<String, FuncId>,
    node_types: HashMap<NodeId, TypeId>,
    res: HashMap<NodeId, Res>,
    caps: Vec<CapEntry>,
    cap_ids: HashMap<String, CapId>,
    /// Every capability something in the program actually calls, which is what
    /// makes an unused `requires` visible.
    cap_used: HashSet<String>,
    /// User-defined record types, by name.
    type_ids: HashMap<String, ObjectId>,
    agents: Vec<AgentInfo>,
    agent_ids: HashMap<String, AgentId>,

    // State for the function currently being checked.
    scopes: Vec<Vec<(String, LocalId)>>,
    locals: Vec<TypeId>,
    ret_ty: Option<TypeId>,
    ret_annotated: bool,
    /// The type a `from_json` in this position has to produce, taken from the
    /// annotation on the binding. `None` means there was none.
    json_target: Option<TypeId>,
}

impl Checker {
    fn new() -> Self {
        Self {
            types: Types::new(),
            diags: Vec::new(),
            fns: Vec::new(),
            fn_ids: HashMap::new(),
            node_types: HashMap::new(),
            res: HashMap::new(),
            caps: Vec::new(),
            cap_ids: HashMap::new(),
            cap_used: HashSet::new(),
            type_ids: HashMap::new(),
            agents: Vec::new(),
            agent_ids: HashMap::new(),
            scopes: Vec::new(),
            locals: Vec::new(),
            ret_ty: None,
            ret_annotated: false,
            json_target: None,
        }
    }

    fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg, Label::new(span, label)));
    }

    fn warning(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::warning(code, msg, Label::new(span, label)));
    }

    fn note(&mut self, note: impl Into<String>) {
        if let Some(last) = self.diags.last_mut() {
            last.notes.push(note.into());
        }
    }

    // ---- pass 0: type declarations ----

    /// Declares every record type, then resolves their fields.
    ///
    /// The two steps are separate so that two types may refer to each other's
    /// names, which means an id has to exist before any field is resolved.
    fn collect_types(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(decl) = item else {
                continue;
            };
            if self.type_ids.contains_key(&decl.name.name) {
                self.error(
                    "E0344",
                    format!("type `{}` is defined more than once", decl.name.name),
                    decl.name.span,
                    "redefined here",
                );
                continue;
            }
            if self.types.by_name(&decl.name.name).is_some()
                || matches!(decl.name.name.as_str(), "List" | "Task")
            {
                self.error(
                    "E0345",
                    format!("`{}` is a built-in type", decl.name.name),
                    decl.name.span,
                    "cannot be redefined",
                );
                continue;
            }
            let id = self.types.declare_object(decl.name.name.clone(), decl.open);
            self.type_ids.insert(decl.name.name.clone(), id);
        }

        for item in &module.items {
            let Item::Type(decl) = item else {
                continue;
            };
            let Some(id) = self.type_ids.get(&decl.name.name).copied() else {
                continue;
            };
            let mut fields: Vec<(String, TypeId)> = Vec::new();
            for field in &decl.fields {
                if fields.iter().any(|(n, _)| *n == field.name.name) {
                    self.error(
                        "E0346",
                        format!("field `{}` is declared twice", field.name.name),
                        field.name.span,
                        "already a field of this type",
                    );
                    continue;
                }
                let ty = self.resolve_type(&field.ty);
                fields.push((field.name.name.clone(), ty));
            }
            self.types.set_object_fields(id, fields);
        }

        self.check_for_recursive_types(module);
    }

    /// A type that contains itself has no finite size.
    ///
    /// A list or a task is a handle, so a cycle through either is fine; the
    /// search stops there.
    fn check_for_recursive_types(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Type(decl) = item else {
                continue;
            };
            let Some(id) = self.type_ids.get(&decl.name.name).copied() else {
                continue;
            };
            let mut seen = vec![id];
            if self.contains_object(&self.types.object(id).fields.clone(), id, &mut seen) {
                self.error(
                    "E0340",
                    format!("type `{}` contains itself", decl.name.name),
                    decl.span,
                    "a value of it would have no finite size",
                );
                self.note("a `List<T>` or a `Task<T>` breaks the cycle, because both are handles");
            }
        }
    }

    fn contains_object(
        &self,
        fields: &[(String, TypeId)],
        target: ObjectId,
        seen: &mut Vec<ObjectId>,
    ) -> bool {
        for (_, ty) in fields {
            let Some(object) = self.types.as_object(*ty) else {
                continue;
            };
            if object == target {
                return true;
            }
            if seen.contains(&object) {
                continue;
            }
            seen.push(object);
            let nested = self.types.object(object).fields.clone();
            if self.contains_object(&nested, target, seen) {
                return true;
            }
        }
        false
    }

    // ---- pass 0: capability grants ----

    /// Builds the module's manifest from its `allow` blocks.
    ///
    /// This runs before anything else, so that a call can be checked against
    /// the manifest no matter where the grant appears in the file.
    fn collect_capabilities(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Allow(decl) = item else {
                continue;
            };
            for grant in &decl.grants {
                let full = grant.path.full_name();
                let Some(sig) = cap::builtin(&full) else {
                    self.error(
                        "E0321",
                        format!("unknown capability `{full}`"),
                        grant.path.span,
                        "no such capability",
                    );
                    self.note(format!("v0.1 has {}", cap::all_names().join(", ")));
                    continue;
                };
                if self.cap_ids.contains_key(&full) {
                    self.error(
                        "E0323",
                        format!("`{full}` is granted more than once"),
                        grant.span,
                        "already granted",
                    );
                    self.note("a capability has one grant, so its manifest entry is unambiguous");
                    continue;
                }
                let constraint = match &grant.constraint {
                    Some(c) => c.clone(),
                    None if sig.requires_constraint => {
                        self.error(
                            "E0322",
                            format!("`{full}` must say what it is limited to"),
                            grant.span,
                            "add the path this grant covers",
                        );
                        self.note(format!(
                            "for example: allow {{ {full} \"/usr/bin/true\"; }}"
                        ));
                        continue;
                    }
                    None => String::new(),
                };
                let pin = match &grant.sha256 {
                    Some(pin) if !sig.accepts_pin => {
                        self.error(
                            "E0327",
                            format!("`{full}` cannot be pinned"),
                            pin.span,
                            "only `process.exec` takes a digest",
                        );
                        self.note("pinning what a capability reads would have to say what the contents must be, which is not what a grant is for");
                        String::new()
                    }
                    Some(pin) if !is_sha256(&pin.text) => {
                        self.error(
                            "E0326",
                            "a pin is a sha256 digest",
                            pin.span,
                            "expected 64 hexadecimal characters",
                        );
                        String::new()
                    }
                    Some(pin) => pin.text.to_ascii_lowercase(),
                    None => String::new(),
                };
                // Only a capability that takes an argument vector can pin
                // what that vector starts with.
                let takes_args = sig.params.last() == Some(&Types::LIST_STR);
                let mut args = Vec::new();
                for arg in &grant.args {
                    if takes_args {
                        args.push(arg.text.clone());
                    } else {
                        self.error(
                            "E0328",
                            format!("`{full}` does not take arguments"),
                            arg.span,
                            "only `process.exec` runs something that can be given any",
                        );
                        break;
                    }
                }
                // `delegable` says the agent answering this program's model
                // calls may use this capability too. It means something only
                // where the manifest does not already bound the authority,
                // which is the `process` family: one argument there can be an
                // entire program. A path scope bounds; `sh -c` does not.
                let delegable = if grant.delegable && !full.starts_with("process.") {
                    self.error(
                        "E0329",
                        format!("`{full}` cannot be delegated"),
                        grant.span,
                        "only a `process` capability takes `delegable`",
                    );
                    self.note(
                        "every other grant already bounds what it allows, so the agent is given \
                         it without the word - see docs/design/authority.md",
                    );
                    false
                } else {
                    grant.delegable
                };
                // `in` and `env` describe a child process, so they mean
                // something only for the capabilities that start one.
                let runs_something = full.starts_with("process.") || full.starts_with("git.");
                // But only a `process` grant may say what the child gets. The
                // whole reason `git` is a capability rather than a
                // `process.run` grant is that the broker decides what git
                // reads - a manifest that could set `GIT_CONFIG_GLOBAL` would
                // be handing that decision straight back.
                if full.starts_with("git.") {
                    if let Some((name, _)) = grant.env.first() {
                        self.error(
                            "E0336",
                            format!("`{full}` decides its own environment"),
                            name.span,
                            "a variable here would change what git reads, which is what this \
                             grant exists to settle; `process.run` is where you say that",
                        );
                    }
                }
                if !runs_something {
                    if let Some(dir) = &grant.dir {
                        self.error(
                            "E0334",
                            format!("`{full}` starts no process"),
                            dir.span,
                            "only a `process` capability takes `in` or `env`",
                        );
                    }
                    if let Some((name, _)) = grant.env.first() {
                        self.error(
                            "E0334",
                            format!("`{full}` starts no process"),
                            name.span,
                            "only a `process` capability takes `in` or `env`",
                        );
                    }
                }
                let dir = match &grant.dir {
                    // A relative directory would be resolved against whatever
                    // shell started `sic`, which is the thing `in` exists to
                    // stop. Refused here rather than at the call, because a
                    // grant is a literal and everything checkable before a run
                    // is checked before it.
                    Some(dir) if !std::path::Path::new(&dir.text).is_absolute() => {
                        self.error(
                            "E0335",
                            "`in` needs an absolute path",
                            dir.span,
                            "a relative one would be decided by the shell",
                        );
                        self.note(
                            "the directory a call runs in is part of what it does, so the \
                             manifest names it the way it names the binary",
                        );
                        String::new()
                    }
                    Some(dir) if runs_something => dir.text.clone(),
                    _ => String::new(),
                };
                let env: Vec<(String, String)> = if runs_something {
                    grant
                        .env
                        .iter()
                        .map(|(name, value)| (name.text.clone(), value.text.clone()))
                        .collect()
                } else {
                    Vec::new()
                };
                // `answers` says what form the program's output takes, so it
                // means something only where there is output to shape. The
                // neighbour of E0334, and refused for the same reason: a
                // clause accepted and ignored is a manifest that says
                // something nothing enforces.
                let answers = match &grant.answers {
                    Some(clause) if !Answers::available_on(&full) => {
                        self.error(
                            "E0337",
                            format!("`{full}` has no output to shape"),
                            clause.span,
                            "only `fs.read`, `process.capture` and `process.run` take `answers`",
                        );
                        self.note(
                            "`process.exec` answers an `Int` and `fs.write` a `Unit`; a `git` \
                             capability answers a value the broker built, and `llm.invoke` says \
                             its shape on the `agent` instead",
                        );
                        Answers::Unsaid
                    }
                    Some(clause) => clause.shape,
                    None => Answers::Unsaid,
                };
                let id = CapId(self.caps.len() as u32);
                self.cap_ids.insert(full.clone(), id);
                self.caps.push(CapEntry {
                    name: full,
                    kind: sig.kind,
                    constraint,
                    pin,
                    args,
                    repeatable: grant.repeatable,
                    delegable,
                    dir,
                    env,
                    answers,
                    params: sig.params.to_vec(),
                    optional_tail: sig.optional_tail,
                    ret: sig.ret,
                });
            }
        }
    }

    // ---- what imported files ask for ----

    /// Checks every `requires` against the grants the program actually made.
    ///
    /// This runs last because it needs to know which capabilities were called,
    /// and a `requires` is a claim about the program as a whole rather than
    /// about one function.
    fn check_requires(&mut self, module: &Module) {
        let mut seen: HashMap<String, Span> = HashMap::new();
        for item in &module.items {
            let Item::Requires(decl) = item else {
                continue;
            };
            for path in &decl.caps {
                let full = path.full_name();
                if cap::builtin(&full).is_none() {
                    self.error(
                        "E0321",
                        format!("unknown capability `{full}`"),
                        path.span,
                        "no such capability",
                    );
                    self.note(format!("v0.1 has {}", cap::all_names().join(", ")));
                    continue;
                }
                if let Some(first) = seen.insert(full.clone(), path.span) {
                    let _ = first;
                    // Two files needing the same capability is ordinary; only
                    // the grant has to be unique.
                    continue;
                }
                if !self.cap_ids.contains_key(&full) {
                    self.error(
                        "E0404",
                        format!("`{full}` is required but not allowed"),
                        path.span,
                        "an imported file needs this",
                    );
                    self.note(format!(
                        "the program decides what it is pointed at: allow {{ {full} \"...\"; }}"
                    ));
                    continue;
                }
                if !self.cap_used.contains(&full) {
                    self.warning(
                        "E0405",
                        format!("`{full}` is required but never called"),
                        path.span,
                        "nothing in the program uses it",
                    );
                }
            }
        }
    }

    /// Declares the module's agents, after types, capabilities and functions.
    fn collect_agents(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Agent(decl) = item else {
                continue;
            };
            if self.agent_ids.contains_key(&decl.name.name) {
                self.error(
                    "E0360",
                    format!("agent `{}` is declared more than once", decl.name.name),
                    decl.name.span,
                    "redeclared here",
                );
                continue;
            }
            if self.fn_ids.contains_key(&decl.name.name) {
                self.error(
                    "E0361",
                    format!("`{}` is already a function", decl.name.name),
                    decl.name.span,
                    "an agent is called like one, so the names would collide",
                );
                continue;
            }

            // An agent is a model call, and a model call is a capability. There
            // is no path to an effect that the manifest does not name.
            self.cap_used.insert("llm.invoke".to_string());
            let Some(cap) = self.cap_ids.get("llm.invoke").copied() else {
                self.error(
                    "E0362",
                    format!("agent `{}` needs `llm.invoke`", decl.name.name),
                    decl.span,
                    "no grant covers talking to a model",
                );
                self.note("declare it: allow { llm.invoke \"the-model\"; }");
                continue;
            };

            let input = match &decl.input {
                Some(ty) => {
                    let resolved = self.resolve_type(ty);
                    if resolved != Types::STR && !self.types.is_error(resolved) {
                        let found = self.types.name(resolved);
                        self.error(
                            "E0363",
                            format!("an agent takes a String, not {found}"),
                            ty.span,
                            "the prompt is text",
                        );
                        self.note("a prompt built from a value needs a way to render one, which v0.1 does not have");
                    }
                    Types::STR
                }
                None => {
                    self.error(
                        "E0364",
                        format!("agent `{}` has no `input`", decl.name.name),
                        decl.span,
                        "add `input: String`",
                    );
                    Types::STR
                }
            };
            let output = match &decl.output {
                Some(ty) => self.resolve_type(ty),
                None => {
                    self.error(
                        "E0364",
                        format!("agent `{}` has no `output`", decl.name.name),
                        decl.span,
                        "add `output: SomeType`",
                    );
                    Types::ERROR
                }
            };

            let id = AgentId(self.agents.len() as u32);
            self.agent_ids.insert(decl.name.name.clone(), id);
            // Numbered from one, so that zero can mean "no conversation" once
            // this reaches the bytecode, where there are no options.
            let conversation = match decl.memory {
                true => Some(self.agents.len() as u32 + 1),
                false => None,
            };
            self.agents.push(AgentInfo {
                name: decl.name.name.clone(),
                input,
                output,
                budget: decl.budget,
                conversation,
                tools: decl.tools,
                deadline_ms: decl.deadline_ms,
                cap,
            });
        }
    }

    // ---- pass 1: signatures ----

    fn collect_signatures(&mut self, module: &Module) {
        for (item_index, item) in module.items.iter().enumerate() {
            let Item::Fn(f) = item else {
                continue;
            };
            if let Some(prev) = self.fn_ids.get(&f.name.name) {
                let prev_span = self.fns[prev.index()].span;
                self.error(
                    "E0304",
                    format!("function `{}` is defined more than once", f.name.name),
                    f.name.span,
                    "redefined here",
                );
                self.diags
                    .last_mut()
                    .unwrap()
                    .secondary
                    .push(Label::new(prev_span, "first defined here"));
                let _ = prev_span;
                continue;
            }

            let params: Vec<TypeId> = f.params.iter().map(|p| self.resolve_type(&p.ty)).collect();
            let ret = f.ret.as_ref().map(|t| self.resolve_type(t));
            let id = FuncId(self.fns.len() as u32);
            self.fn_ids.insert(f.name.name.clone(), id);
            self.fns.push(FnState {
                name: f.name.name.clone(),
                local_types: params.clone(),
                params,
                ret,
                item_index,
                span: f.span,
            });
        }
    }

    fn resolve_type(&mut self, t: &TypeExpr) -> TypeId {
        // `Task` is the only type with an argument in v0.1. A list type is a
        // separate piece of work.
        // `LLM<T>` and `HumanApproved<T>` can be written in a signature, which
        // is the useful direction: a function says what it will accept.
        if let Some(kind) = TrustKind::from_name(&t.name.name) {
            if t.args.len() != 1 {
                self.error(
                    "E0310",
                    format!("`{}` takes exactly one type argument", t.name.name),
                    t.span,
                    "write `LLM<T>`",
                );
                return Types::ERROR;
            }
            let inner = self.resolve_type(&t.args[0]);
            return self.types.trust(kind, inner);
        }
        if t.name.name == "List" {
            if t.args.len() != 1 {
                self.error(
                    "E0310",
                    "`List` takes exactly one type argument",
                    t.span,
                    "write `List<T>`",
                );
                return Types::ERROR;
            }
            let element = self.resolve_type(&t.args[0]);
            return self.types.list(element);
        }
        if t.name.name == "Task" {
            if t.args.len() != 1 {
                self.error(
                    "E0310",
                    "`Task` takes exactly one type argument",
                    t.span,
                    "write `Task<T>`",
                );
                return Types::ERROR;
            }
            let inner = self.resolve_type(&t.args[0]);
            return self.types.task(inner);
        }
        if !t.args.is_empty() {
            self.error(
                "E0310",
                format!("`{}` takes no type arguments in v0.1", t.name.name),
                t.span,
                "type arguments are not supported yet",
            );
            return Types::ERROR;
        }
        if let Some(id) = self.types.by_name(&t.name.name) {
            return id;
        }
        if let Some(object) = self.type_ids.get(&t.name.name).copied() {
            return self.types.intern(Type::Object(object));
        }
        self.error(
            "E0310",
            format!("unknown type `{}`", t.name.name),
            t.span,
            "not a known type",
        );
        self.note("v0.1 has Unit, Bool, Int, Float, String, List<T>, Task<T> and the types this module declares");
        Types::ERROR
    }

    // ---- pass 2: bodies ----

    fn check_bodies(&mut self, module: &Module) {
        for idx in 0..self.fns.len() {
            let item_index = self.fns[idx].item_index;
            let Item::Fn(decl) = &module.items[item_index] else {
                unreachable!("a function's item index must name a function");
            };
            self.check_fn(FuncId(idx as u32), decl);
        }
    }

    fn check_fn(&mut self, id: FuncId, decl: &FnDecl) {
        let state = &self.fns[id.index()];
        self.locals = state.params.clone();
        self.ret_ty = state.ret;
        self.ret_annotated = state.ret.is_some();

        self.scopes = vec![Vec::new()];
        for (i, p) in decl.params.iter().enumerate() {
            let slot = LocalId(i as u32);
            self.declare(&p.name.name, slot);
            self.res.insert(p.id, Res::Local(slot));
        }

        self.check_block(&decl.body);

        // An unannotated function with no `return` returns Unit.
        let ret = self.ret_ty.unwrap_or(Types::UNIT);
        if ret != Types::UNIT && !self.types.is_error(ret) && !always_returns(&decl.body) {
            let name = self.types.name(ret);
            self.error(
                "E0307",
                format!("`{}` must return {name} on every path", decl.name.name),
                decl.body.span,
                "control can reach the end of the body without returning",
            );
        }

        // A task means nothing outside the run that owns it, so it cannot be
        // what a run produces.
        if decl.name.name == "main" && self.types.task_output(ret).is_some() {
            let name = self.types.name(ret);
            self.error(
                "E0331",
                format!("`main` cannot return {name}"),
                decl.span,
                "a task has no meaning outside its run",
            );
            self.note("await it before returning");
        }

        let locals = std::mem::take(&mut self.locals);
        let state = &mut self.fns[id.index()];
        state.ret = Some(ret);
        state.local_types = locals;
        self.scopes.clear();
    }

    fn declare(&mut self, name: &str, slot: LocalId) {
        self.scopes
            .last_mut()
            .expect("a scope must be open")
            .push((name.to_string(), slot));
    }

    /// Looks up a name in the innermost scope first, so an inner binding
    /// shadows an outer one.
    fn lookup(&self, name: &str) -> Option<LocalId> {
        for scope in self.scopes.iter().rev() {
            for (n, slot) in scope.iter().rev() {
                if n == name {
                    return Some(*slot);
                }
            }
        }
        None
    }

    fn new_local(&mut self, ty: TypeId) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(ty);
        id
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(Vec::new());
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let {
                id,
                name,
                ty,
                init,
                span,
            } => {
                let annotated = ty.as_ref().map(|t| self.resolve_type(t));
                let from_annotation = annotated.and_then(|want| self.empty_list_of(init, want));
                let init_ty = match from_annotation {
                    Some(want) => want,
                    None => {
                        // The annotation is what a `from_json` in this position
                        // produces.
                        let saved = self.json_target.replace(annotated.unwrap_or(Types::ERROR));
                        if annotated.is_none() {
                            self.json_target = None;
                        }
                        let ty = self.check_expr(init);
                        self.json_target = saved;
                        ty
                    }
                };
                let slot_ty = match annotated {
                    Some(want) => {
                        self.expect_type(want, init_ty, init.span, "this initializer");
                        want
                    }
                    None => init_ty,
                };
                if slot_ty == Types::UNIT {
                    self.error(
                        "E0311",
                        format!("`{}` would have type Unit", name.name),
                        *span,
                        "a binding must hold a value",
                    );
                }
                let slot = self.new_local(slot_ty);
                self.declare(&name.name, slot);
                self.res.insert(*id, Res::Local(slot));
            }
            Stmt::Return { value, span, .. } => {
                let found = match value {
                    // The function's own return type is written down, so an
                    // empty list here has one too.
                    Some(e) => match self.ret_ty.and_then(|want| self.empty_list_of(e, want)) {
                        Some(want) => want,
                        None => self.check_expr(e),
                    },
                    None => Types::UNIT,
                };
                let span = value.as_ref().map(|e| e.span).unwrap_or(*span);
                match self.ret_ty {
                    Some(want) => self.expect_type(want, found, span, "this value"),
                    None => {
                        // First `return` of an unannotated function fixes its type.
                        self.ret_ty = Some(found);
                    }
                }
            }
            Stmt::If(if_stmt) => self.check_if(if_stmt),
            Stmt::For(for_stmt) => self.check_for(for_stmt),
            Stmt::Expr { expr, .. } => {
                self.check_expr(expr);
            }
            // A message is text, whatever produced it. Any provenance is
            // allowed and erased: logging reaches nothing outside the run's own
            // account of itself, so the rule that stops a value from deciding
            // what runs has nothing to say here. `docs/design/logging.md` says
            // what changes the day `Secret<T>` exists.
            Stmt::Log { message, .. } => {
                let found = self.check_expr(message);
                let erased = self.types.untrusted_deep(found);
                self.expect_type(Types::STR, erased, message.span, "this message");
            }
        }
    }

    /// `for x in xs { ... }`.
    ///
    /// The binding is a local like any other, scoped to the body, so it leaves
    /// no name behind after the loop. There is nothing to check about how many
    /// times the body runs: the count is `len(xs)` and the list cannot change
    /// while it is being walked, because nothing in the language mutates.
    fn check_for(&mut self, for_stmt: &ForStmt) {
        let iter_ty = self.check_expr(&for_stmt.iter);
        let element = match self.element_type(iter_ty) {
            Some(element) => element,
            None if self.types.is_error(iter_ty) => Types::ERROR,
            None => {
                // Named the way E0351 names it: what a list of these would
                // hold, rather than where the value came from.
                let found = self.types.name(self.types.untrusted(iter_ty));
                self.error(
                    "E0354",
                    format!("{found} cannot be walked with `for`"),
                    for_stmt.iter.span,
                    "only a `List<T>` can be walked",
                );
                Types::ERROR
            }
        };
        // The binding and the body share one scope, so the loop variable is
        // gone at the closing brace and an inner `let` of the same name still
        // shadows it.
        self.scopes.push(Vec::new());
        let slot = self.new_local(element);
        self.declare(&for_stmt.var.name, slot);
        self.res.insert(for_stmt.id, Res::Local(slot));
        for stmt in &for_stmt.body.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    /// The type of one element of a list, or `None` when this is not a list.
    ///
    /// An element of a model's answer is still the model's answer, the same way
    /// a field is. `xs[i]` and `for x in xs` reach an element by the same route,
    /// so they agree about its provenance by sharing this rather than by both
    /// remembering to. `docs/design/trust.md` §2a is what they agree with.
    fn element_type(&mut self, base_ty: TypeId) -> Option<TypeId> {
        let provenance = self.types.trust_of(base_ty).map(|(kind, _)| kind);
        let base_ty = self.types.untrusted(base_ty);
        let element = self.types.list_element(base_ty)?;
        Some(match provenance {
            Some(kind) => self.types.trust(kind, element),
            None => element,
        })
    }

    fn check_if(&mut self, if_stmt: &IfStmt) {
        let cond = self.check_expr(&if_stmt.cond);
        self.expect_type(Types::BOOL, cond, if_stmt.cond.span, "this condition");
        self.check_block(&if_stmt.then_block);
        match if_stmt.else_branch.as_deref() {
            Some(ElseBranch::Block(b)) => self.check_block(b),
            Some(ElseBranch::If(inner)) => self.check_if(inner),
            None => {}
        }
    }

    /// Reports a mismatch unless either side is already an error.
    fn expect_type(&mut self, want: TypeId, found: TypeId, span: Span, what: &str) {
        if want == found || self.types.is_error(want) || self.types.is_error(found) {
            return;
        }
        let (w, f) = (self.types.name(want), self.types.name(found));
        self.error(
            "E0301",
            format!("expected {w}, found {f}"),
            span,
            format!("{what} has type {f}"),
        );
    }

    fn check_expr(&mut self, e: &Expr) -> TypeId {
        let ty = match &e.kind {
            ExprKind::Int(_) => Types::INT,
            ExprKind::Float(_) => Types::FLOAT,
            ExprKind::Bool(_) => Types::BOOL,
            ExprKind::Str(_) => Types::STR,
            ExprKind::Null => {
                self.error(
                    "E0312",
                    "`null` has no type in v0.1",
                    e.span,
                    "there is no optional type yet",
                );
                Types::ERROR
            }
            ExprKind::Path(name) => self.check_path(e.id, name),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, e.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, e.span),
            ExprKind::Call {
                callee,
                args,
                policy,
            } => self.check_call(callee, args, policy, e.span),
            ExprKind::Spawn { callee, args } => self.check_spawn(callee, args, e.span),
            ExprKind::Struct { name, fields } => self.check_struct(name, fields, e.span),
            ExprKind::List { elements } => self.check_list(elements, e.span),
            ExprKind::Index { base, index } => self.check_index(base, index, e.span),
            ExprKind::Await { task } => self.check_await(task, e.span),
            ExprKind::Field { base, name } => {
                if let Some(full) = self.capability_name(base, name) {
                    self.error(
                        "E0325",
                        format!("`{full}` is a capability and must be called"),
                        e.span,
                        "a capability is not a value",
                    );
                    return_error(&mut self.node_types, e.id)
                } else {
                    self.check_field_access(base, name, e.span)
                }
            }
            ExprKind::Error => Types::ERROR,
        };
        self.node_types.insert(e.id, ty);
        ty
    }

    fn check_path(&mut self, node: NodeId, name: &Ident) -> TypeId {
        if let Some(slot) = self.lookup(&name.name) {
            self.res.insert(node, Res::Local(slot));
            return self.locals[slot.index()];
        }
        if let Some(id) = self.fn_ids.get(&name.name).copied() {
            self.res.insert(node, Res::Fn(id));
            let sig = crate::ty::FnSig {
                params: self.fns[id.index()].params.clone(),
                ret: self.fns[id.index()].ret.unwrap_or(Types::ERROR),
            };
            let sig_id = self.types.add_sig(sig);
            return self.types.intern(Type::Fn(sig_id));
        }
        self.error(
            "E0300",
            format!("cannot find `{}`", name.name),
            name.span,
            "not defined in this scope",
        );
        Types::ERROR
    }

    fn check_unary(&mut self, op: UnOp, operand: &Expr, span: Span) -> TypeId {
        let ty = self.check_expr(operand);
        if self.types.is_error(ty) {
            return Types::ERROR;
        }
        if self.reject_trust(ty, operand.span, "an operand") {
            return Types::ERROR;
        }
        let want = match op {
            UnOp::Neg => Types::INT,
            UnOp::Not => Types::BOOL,
        };
        if ty != want {
            let (t, w) = (self.types.name(ty), self.types.name(want));
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {t}", op.text()),
                span,
                format!("expected {w}"),
            );
            if op == UnOp::Neg && ty == Types::FLOAT {
                self.note("v0.1 has integer arithmetic only");
            }
            return Types::ERROR;
        }
        want
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> TypeId {
        let l = self.check_expr(lhs);
        let r = self.check_expr(rhs);
        if self.types.is_error(l) || self.types.is_error(r) {
            return Types::ERROR;
        }
        // Joining two strings is the one operator a labelled value may be an
        // operand of, and `check_concat` is where the reason is written down.
        // It has to come first, because the rule below is what it is an
        // exception to.
        if op == BinOp::Add
            && self.types.untrusted(l) == Types::STR
            && self.types.untrusted(r) == Types::STR
        {
            return self.check_concat(l, r, span);
        }
        // Arithmetic on a value whose provenance matters is exactly where
        // provenance gets lost - but a comparison is not arithmetic. It answers
        // a `Bool` *about* its operands, and a `Bool` cannot be one of them.
        // `docs/design/trust.md` §2a has the rule and what it costs.
        if !self.asks_a_question(op, l, r)
            && (self.reject_trust(l, lhs.span, "an operand")
                || self.reject_trust(r, rhs.span, "an operand"))
        {
            return Types::ERROR;
        }
        // A comparison is where a label stops, so what is compared is the type
        // underneath it. Two operands labelled differently are compared without
        // complaint, which is where this parts company with `+`: a join has to
        // name where its result came from (E0375), and an answer to a question
        // came from nowhere.
        let (l, r) = (self.types.untrusted(l), self.types.untrusted(r));
        if l != r {
            let (ln, rn) = (self.types.name(l), self.types.name(r));
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {ln} and {rn}", op.text()),
                span,
                "both operands must have the same type",
            );
            self.note("there are no implicit conversions");
            return Types::ERROR;
        }

        // Both sides share the type `l` from here on.
        let ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => l == Types::INT,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => l == Types::INT,
            // Equality is byte equality of the interned string: not case
            // folding, not normalization, not trimming. `"main" == "Main"` is
            // false. Ordering is left out, because `<` on strings needs a
            // collation decision and nothing has asked for one.
            BinOp::Eq | BinOp::Ne => l == Types::INT || l == Types::BOOL || l == Types::STR,
            BinOp::And | BinOp::Or => l == Types::BOOL,
        };
        if !ok {
            let name = self.types.name(l);
            self.error(
                "E0303",
                format!("`{}` cannot be applied to {name}", op.text()),
                span,
                format!("no such operator for {name}"),
            );
            if l == Types::FLOAT {
                self.note("v0.1 supports arithmetic and comparison on Int only");
            } else if l == Types::STR {
                self.note("v0.1 joins String with `+`, and compares it with `==` and `!=`");
            }
            return Types::ERROR;
        }

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => l,
            _ => Types::BOOL,
        }
    }

    /// Whether `op` answers a `Bool` *about* its operands rather than a value
    /// of their own kind - which is the whole of what decides whether a
    /// labelled operand is refused.
    ///
    /// The criterion is not "the result is a `Bool`". It is that the result
    /// cannot be an operand: `x == true` and `x != false` hand back exactly the
    /// `Bool` they were given, which is the laundering shape `x + 0` is, spelled
    /// with a different operator. Comparing an `Int` or a `String` cannot do
    /// that - one bit comes back and the value stays where it was.
    ///
    /// So this is the same criterion `check_concat` answers the other way. An
    /// operator whose result has its operands' type either refuses a label
    /// (arithmetic, negation, the connectives) or carries it (joining two
    /// strings); an operator whose result cannot hold the operand is a question,
    /// and a question may be asked about a labelled value. See
    /// `docs/design/trust.md` §2a.
    fn asks_a_question(&self, op: BinOp, l: TypeId, r: TypeId) -> bool {
        match op {
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => true,
            BinOp::Eq | BinOp::Ne => {
                self.types.untrusted(l) != Types::BOOL && self.types.untrusted(r) != Types::BOOL
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => false,
            BinOp::And | BinOp::Or => false,
        }
    }

    /// `a + b` on two strings, and what the result came from.
    ///
    /// Every other operator refuses a labelled operand (E0371), and the reason
    /// `docs/design/trust.md` §2a gives is that an operator takes a labelled
    /// value and answers an unlabelled one - so the label would come off.
    /// Joining two strings does not do that. It answers a labelled value, which
    /// puts it beside reading a field and indexing a list rather than beside
    /// arithmetic: the program never gets back something plain to compute with,
    /// and `"" + tainted` has no more reach than `tainted` did.
    ///
    /// Two operands with *different* labels are refused. There is no order
    /// between "a model said it" and "a program printed it" to pick a winner
    /// by, and inventing one would be a lattice built for a program nobody has
    /// written. `Types::trust` already says a value has one origin; a join of
    /// two origins has none that can be named, so it is not made.
    fn check_concat(&mut self, l: TypeId, r: TypeId, span: Span) -> TypeId {
        match (self.types.trust_of(l), self.types.trust_of(r)) {
            (None, None) => Types::STR,
            (Some((kind, _)), None) | (None, Some((kind, _))) => self.types.trust(kind, Types::STR),
            (Some((left, _)), Some((right, _))) if left == right => {
                self.types.trust(left, Types::STR)
            }
            (Some(_), Some(_)) => {
                let (ln, rn) = (self.types.name(l), self.types.name(r));
                self.error(
                    "E0375",
                    format!("`+` cannot join {ln} with {rn}"),
                    span,
                    "the result would have come from two places, and a value comes from one",
                );
                self.note("join a labelled value with a literal, or with one from the same place");
                Types::ERROR
            }
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        policy: &CallPolicy,
        span: Span,
    ) -> TypeId {
        // `fs.read(..)` parses as a call whose callee is a field access. That
        // shape is a capability call and nothing else, because there are no
        // object types to take a method from.
        if let ExprKind::Field { base, name } = &callee.kind {
            return self.check_cap_call(callee, base, name, args, policy, span);
        }
        // Retrying a pure function computes the same answer again, and a
        // deadline on one measures nothing that can be waited for.
        //
        // An agent call is the exception a reader will meet: it is written as a
        // function call and it does have an effect, so the note says where the
        // number it was reaching for actually goes rather than telling them
        // there is nothing to wait for.
        if let Some(policy_span) = policy.span {
            let is_agent = matches!(&callee.kind, ExprKind::Path(name)
                if self.agents.iter().any(|a| a.name == name.name));
            self.error(
                "E0330",
                "`retry` and `timeout` apply to capability calls only",
                policy_span,
                match is_agent {
                    true => "this is an agent call",
                    false => "this is a function call",
                },
            );
            if is_agent {
                self.note(
                    "an agent is bounded in its declaration: `budget` for model calls, \
                     `tools` for tool uses, `deadline` for wall clock",
                );
            } else {
                self.note("a function has no effect to retry or to wait for");
            }
        }
        let ExprKind::Path(name) = &callee.kind else {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0305",
                "only a named function can be called in v0.1",
                callee.span,
                "not a function name",
            );
            return Types::ERROR;
        };

        // A local binding shadows a function, and no local can hold a function
        // in v0.1, so this is a call of something that is not callable.
        if self.lookup(&name.name).is_some() {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0305",
                format!("`{}` is a variable, not a function", name.name),
                callee.span,
                "cannot be called",
            );
            return Types::ERROR;
        }

        if let Some(agent) = self.agent_ids.get(&name.name).copied() {
            return self.check_agent_call(callee, agent, args, span);
        }
        let Some(id) = self.fn_ids.get(&name.name).copied() else {
            // The built-in functions. They are looked up last, so a module
            // that defines its own `len` gets that one.
            if name.name == "len" {
                self.res.insert(callee.id, Res::Builtin(Builtin::Len));
                return self.check_len(args, span);
            }
            if name.name == "approve" {
                self.res.insert(callee.id, Res::Builtin(Builtin::Approve));
                return self.check_approve(args, span);
            }
            if name.name == "choose" {
                self.res.insert(callee.id, Res::Builtin(Builtin::Choose));
                return self.check_choose(args, span);
            }
            if name.name == "from_json" {
                self.res.insert(callee.id, Res::Builtin(Builtin::FromJson));
                return self.check_from_json(args, span);
            }
            if name.name == "contains" {
                self.res.insert(callee.id, Res::Builtin(Builtin::Contains));
                return self.check_str_test("contains", ["haystack", "needle"], args, span);
            }
            if name.name == "starts_with" {
                self.res
                    .insert(callee.id, Res::Builtin(Builtin::StartsWith));
                return self.check_str_test("starts_with", ["string", "prefix"], args, span);
            }
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0300",
                format!("cannot find function `{}`", name.name),
                name.span,
                "not defined in this module",
            );
            return Types::ERROR;
        };
        self.res.insert(callee.id, Res::Fn(id));

        let params = self.fns[id.index()].params.clone();
        let ret = self.fns[id.index()].ret;

        if args.len() != params.len() {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!(
                    "`{}` takes {} argument(s) but {} were given",
                    name.name,
                    params.len(),
                    args.len()
                ),
                span,
                "wrong number of arguments",
            );
            return ret.unwrap_or(Types::ERROR);
        }

        for (arg, want) in args.iter().zip(params) {
            let found = match self.empty_list_of(arg, want) {
                Some(ty) => ty,
                None => self.check_expr(arg),
            };
            self.expect_type(want, found, arg.span, "this argument");
        }

        match ret {
            Some(ty) => ty,
            None => {
                // Its return type is still being inferred, which means the call
                // is a forward or recursive reference.
                self.error(
                    "E0306",
                    format!("`{}` needs a return type annotation", name.name),
                    name.span,
                    "called before its type is known",
                );
                self.note("annotate it with `-> Type`, or define it before this call");
                Types::ERROR
            }
        }
    }

    /// The capability a `base.name` expression could name, if `base` is a plain
    /// identifier that no local binding shadows.
    fn capability_name(&self, base: &Expr, name: &Ident) -> Option<String> {
        let ExprKind::Path(ns) = &base.kind else {
            return None;
        };
        if self.lookup(&ns.name).is_some() {
            return None;
        }
        let full = format!("{}.{}", ns.name, name.name);
        cap::builtin(&full).map(|_| full)
    }

    fn check_cap_call(
        &mut self,
        callee: &Expr,
        base: &Expr,
        name: &Ident,
        args: &[Expr],
        policy: &CallPolicy,
        span: Span,
    ) -> TypeId {
        let Some(full) = self.capability_name(base, name) else {
            for a in args {
                self.check_expr(a);
            }
            // Either the namespace is a local binding, or there is no such
            // capability; both mean this cannot be a capability call.
            if let ExprKind::Path(ns) = &base.kind {
                let attempted = format!("{}.{}", ns.name, name.name);
                if self.lookup(&ns.name).is_none() {
                    self.error(
                        "E0324",
                        format!("unknown capability `{attempted}`"),
                        span,
                        "no such capability",
                    );
                    self.note(format!("v0.1 has {}", cap::all_names().join(", ")));
                    return Types::ERROR;
                }
            }
            self.check_expr(base);
            self.error(
                "E0308",
                "field access is not supported in v0.1",
                callee.span,
                "there are no object types yet",
            );
            return Types::ERROR;
        };

        let Some(id) = self.cap_ids.get(&full).copied() else {
            for a in args {
                self.check_expr(a);
            }
            // The capability exists but the module never granted it. Declaring
            // it is the fix, which is why this is an error at compile time
            // rather than a refusal at run time.
            self.error(
                "E0320",
                format!("`{full}` is not allowed by this module"),
                span,
                "no grant covers this call",
            );
            self.note(format!("declare it: allow {{ {full} \"...\"; }}"));
            return Types::ERROR;
        };
        self.res.insert(callee.id, Res::Cap(id));
        self.cap_used.insert(full.clone());

        let entry = &self.caps[id.index()];
        // Retrying performs the effect again - that is what retrying is - so a
        // program may only ask for it where the manifest says performing it
        // twice is the same as performing it once. The claim belongs to the
        // manifest because that is where claims about effects live and where
        // `sic plan` reads them from, and it is opt-in because whoever has not
        // heard of it would otherwise find out by deploying twice.
        let repeatable = entry.repeatable;
        if !repeatable && policy.attempts.is_some_and(|n| n > 1) {
            let at = policy.span.unwrap_or(span);
            self.error(
                "E0374",
                format!("`{full}` may not be retried"),
                at,
                "this grant does not say the effect can be repeated",
            );
            self.note(format!(
                "retrying performs the effect again; if that is safe, say so: \
                 allow {{ {full} \"...\" repeatable; }}"
            ));
        }
        let entry = &self.caps[id.index()];
        let (params, ret) = (entry.params.clone(), entry.ret);
        // An optional tail means the call may stop one short of the signature.
        let least = params.len() - usize::from(entry.optional_tail);
        if args.len() < least || args.len() > params.len() {
            for a in args {
                self.check_expr(a);
            }
            let wanted = if least == params.len() {
                format!("{least}")
            } else {
                format!("{least} or {}", params.len())
            };
            self.error(
                "E0302",
                format!(
                    "`{full}` takes {wanted} argument(s) but {} were given",
                    args.len()
                ),
                span,
                "wrong number of arguments",
            );
            return ret;
        }
        let kind = entry.kind;
        for (arg, want) in args.iter().zip(params) {
            let found = match self.empty_list_of(arg, want) {
                Some(ty) => ty,
                None => self.check_expr(arg),
            };
            match self.types.trust_of(found) {
                // A value nobody signed off must not reach a capability that
                // changes something. Reading or asking is fine - asking a model
                // about a model's answer is ordinary - which is why the rule is
                // about the capability's kind rather than about the value.
                Some((kind_of @ (TrustKind::Llm | TrustKind::Observed), _))
                    if matches!(kind, sic_core::CapKind::Write | sic_core::CapKind::Exec) =>
                {
                    let name = self.types.name(found);
                    let source = match kind_of {
                        TrustKind::Observed => "a program printed it",
                        _ => "this came from a model",
                    };
                    self.error(
                        "E0372",
                        format!("{name} cannot be passed to `{full}`"),
                        arg.span,
                        format!("`{full}` changes something, and {source}"),
                    );
                    self.note("`approve(question, value)` turns it into one a person signed off");
                }
                // A capability erases trust, so what is compared is the type
                // underneath it - at any depth, or an approved value could not
                // be put in an argument vector at all.
                _ => {
                    let erased = self.types.untrusted_deep(found);
                    self.expect_type(want, erased, arg.span, "this argument")
                }
            }
        }
        ret
    }

    /// `spawn f(args)`: the arguments are checked as for a call, and the result
    /// is a task producing what `f` returns.
    fn check_spawn(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        if let ExprKind::Field { base, name } = &callee.kind {
            for a in args {
                self.check_expr(a);
            }
            let attempted = match &base.kind {
                ExprKind::Path(ns) => format!("{}.{}", ns.name, name.name),
                _ => name.name.clone(),
            };
            // Spawning an effect would mean two capability calls in flight at
            // once, which is a broker change rather than a language one.
            self.error(
                "E0332",
                format!("`{attempted}` is a capability and cannot be spawned"),
                span,
                "only a function can be spawned",
            );
            return Types::ERROR;
        }

        let ret = self.check_call(callee, args, &CallPolicy::default(), span);
        if self.types.is_error(ret) {
            return Types::ERROR;
        }
        self.types.task(ret)
    }

    fn check_await(&mut self, task: &Expr, span: Span) -> TypeId {
        let ty = self.check_expr(task);
        if self.types.is_error(ty) {
            return Types::ERROR;
        }
        match self.types.task_output(ty) {
            Some(output) => output,
            None => {
                let name = self.types.name(ty);
                self.error(
                    "E0333",
                    format!("`await` needs a task, found {name}"),
                    span,
                    "only a `Task<T>` can be awaited",
                );
                Types::ERROR
            }
        }
    }

    /// `len(xs)` for a list or a string.
    fn check_len(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 1 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!("`len` takes 1 argument but {} were given", args.len()),
                span,
                "wrong number of arguments",
            );
            return Types::INT;
        }
        // How long something is says nothing about where it came from, so a
        // length is a plain Int.
        let found = self.check_expr(&args[0]);
        let ty = self.types.untrusted(found);
        if self.types.is_error(ty) {
            return Types::INT;
        }
        if ty != Types::STR && self.types.list_element(ty).is_none() {
            let found = self.types.name(ty);
            self.error(
                "E0352",
                format!("`len` cannot be applied to {found}"),
                args[0].span,
                "expected a `List<T>` or a String",
            );
        }
        Types::INT
    }

    /// `contains(haystack, needle)` and `starts_with(string, prefix)`: two
    /// strings in, a `Bool` out.
    ///
    /// One procedure because the rule is one rule. The two differ in where the
    /// needle is allowed to be, which is a question for the VM; the types, the
    /// arity and the trust decision are the same, and a second copy of them is
    /// a second place for them to drift apart.
    ///
    /// **The label comes off.** A labelled string may be asked either
    /// question, and the answer is a plain `Bool`, for the reason
    /// `docs/design/trust.md` §2a gives for `len`: a branch is not an effect,
    /// and a `Bool` cannot be written to a file, passed to `exec`, or turned
    /// back into the string it was asked about. It is a wider channel than
    /// `len` - see §2a, which says how much wider and what that costs.
    fn check_str_test(&mut self, name: &str, what: [&str; 2], args: &[Expr], span: Span) -> TypeId {
        if args.len() != 2 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!("`{name}` takes 2 arguments but {} were given", args.len()),
                span,
                format!("write `{name}({}, {})`", what[0], what[1]),
            );
            return Types::BOOL;
        }
        for (arg, what) in args.iter().zip(what) {
            let found = self.check_expr(arg);
            let found = self.types.untrusted(found);
            self.expect_type(Types::STR, found, arg.span, &format!("this {what}"));
        }
        Types::BOOL
    }

    /// An agent is called like a function: one argument in, its output type out.
    fn check_agent_call(
        &mut self,
        callee: &Expr,
        agent: AgentId,
        args: &[Expr],
        span: Span,
    ) -> TypeId {
        let info = self.agents[agent.index()].clone();
        self.res.insert(callee.id, Res::Agent(agent));
        if args.len() != 1 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!(
                    "agent `{}` takes 1 argument but {} were given",
                    info.name,
                    args.len()
                ),
                span,
                "wrong number of arguments",
            );
            return info.output;
        }
        let found = self.check_expr(&args[0]);
        // Asking a model about a model's answer is ordinary, so a prompt keeps
        // its provenance without being refused for it. An agent is an
        // `llm.invoke`, and invoking changes nothing.
        let found = self.types.untrusted(found);
        self.expect_type(info.input, found, args[0].span, "this prompt");
        // The model produced it, and that is part of what it is.
        self.types.trust(TrustKind::Llm, info.output)
    }

    /// `approve(question, value)`: asks a person, and fails the run if the
    /// answer is no.
    ///
    /// There is no third outcome to return. Without an option type, "approved
    /// or not" would have to be a `Bool` beside the value, and nothing would
    /// stop the program from ignoring it.
    fn check_approve(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 2 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!("`approve` takes 2 arguments but {} were given", args.len()),
                span,
                "write `approve(question, value)`",
            );
            return Types::ERROR;
        }
        let question = self.check_expr(&args[0]);
        self.expect_type(Types::STR, question, args[0].span, "this question");
        let value = self.check_expr(&args[1]);

        // Asking a person is an effect like any other.
        self.cap_used.insert("human.approve".to_string());
        if !self.cap_ids.contains_key("human.approve") {
            self.error(
                "E0370",
                "`approve` needs `human.approve`",
                span,
                "no grant covers asking a person",
            );
            self.note("declare it: allow { human.approve \"what this covers\"; }");
            return Types::ERROR;
        }
        if self.types.is_error(value) {
            return Types::ERROR;
        }
        // The value is shown to whoever is asked, so it has to be something
        // that can be shown. A task is the one thing a run holds that is not:
        // it is a computation in this run and means nothing outside it, which
        // is the same reason it cannot cross to the broker.
        if let Some(what) = self.unshowable(value) {
            self.error(
                "E0376",
                format!("`{what}` cannot be shown to whoever is asked"),
                args[1].span,
                "`approve` shows this value to a person",
            );
            self.note("await the task and approve what it produced".to_string());
            return Types::ERROR;
        }
        self.types.trust(TrustKind::HumanApproved, value)
    }

    /// The name of the part of a type that cannot be rendered for a person, if
    /// there is one.
    ///
    /// Recursive because a list of tasks is no more showable than a task, and
    /// a record's fields are declared in source where a future type could be.
    fn unshowable(&self, ty: TypeId) -> Option<String> {
        let ty = self.types.untrusted(ty);
        match self.types.get(ty) {
            Type::Task(_) | Type::Fn(_) => Some(self.types.name(ty)),
            Type::List(element) => self.unshowable(*element),
            Type::Object(object) => self
                .types
                .object(*object)
                .fields
                .clone()
                .iter()
                .find_map(|(_, field)| self.unshowable(*field)),
            _ => None,
        }
    }

    /// `choose(question, options)`: ask a person which one, and hand back the
    /// option they picked.
    ///
    /// The alternatives are strings because a person reads them. What comes
    /// back is one of them - the capability answers with an index, and the VM
    /// reads the value out of the list this call already built.
    fn check_choose(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 2 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!("`choose` takes 2 arguments but {} were given", args.len()),
                span,
                "write `choose(question, options)`",
            );
            return Types::ERROR;
        }
        let question = self.check_expr(&args[0]);
        self.expect_type(Types::STR, question, args[0].span, "this question");
        // `choose` is the one builtin whose parameter is a concrete list type,
        // so it is the one the rule in `empty_list_of` reaches. `len` and
        // `approve` take a list of anything, which is nothing to take.
        let options = match self.empty_list_of(&args[1], Types::LIST_STR) {
            Some(ty) => ty,
            None => self.check_expr(&args[1]),
        };
        self.expect_type(Types::LIST_STR, options, args[1].span, "these options");

        // Asking a person is an effect like any other.
        self.cap_used.insert("human.choose".to_string());
        if !self.cap_ids.contains_key("human.choose") {
            self.error(
                "E0373",
                "`choose` needs `human.choose`",
                span,
                "no grant covers asking a person to decide",
            );
            self.note("declare it: allow { human.choose \"what this covers\"; }");
            return Types::ERROR;
        }
        if self.types.is_error(options) {
            return Types::ERROR;
        }
        self.types.trust(TrustKind::HumanChosen, Types::STR)
    }

    /// `from_json(text)`, whose result type is the annotation on the binding.
    ///
    /// There is nothing in the call itself to infer a type from, and inventing
    /// one would move the error from the model boundary to wherever the value
    /// is first used.
    fn check_from_json(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 1 {
            for a in args {
                self.check_expr(a);
            }
            self.error(
                "E0302",
                format!("`from_json` takes 1 argument but {} were given", args.len()),
                span,
                "wrong number of arguments",
            );
            return Types::ERROR;
        }
        let text = self.check_expr(&args[0]);
        // A document may arrive labelled - `from_json(llm.invoke(...))` is the
        // manual spelling of what an `agent` declaration does - so the label is
        // taken off to check the argument and put back on the result. Reading
        // a model's answer into a shape does not stop it being the model's
        // answer, and a `from_json` that dropped the label would be the second
        // door out of §2's rule.
        let document = self.types.trust_of(text).map(|(kind, _)| kind);
        self.expect_type(
            Types::STR,
            self.types.untrusted(text),
            args[0].span,
            "this document",
        );

        match self.json_target {
            Some(ty) => match document {
                Some(kind) => self.types.trust(kind, ty),
                None => ty,
            },
            None => {
                self.error(
                    "E0353",
                    "`from_json` needs to know what type to produce",
                    span,
                    "annotate the binding it initializes",
                );
                self.note("write `let d: Diagnosis = from_json(text);`");
                Types::ERROR
            }
        }
    }

    fn check_struct(&mut self, name: &Ident, fields: &[FieldInit], span: Span) -> TypeId {
        let Some(object) = self.type_ids.get(&name.name).copied() else {
            for field in fields {
                self.check_expr(&field.value);
            }
            self.error(
                "E0347",
                format!("`{}` is not a record type", name.name),
                name.span,
                "no such type in this module",
            );
            return Types::ERROR;
        };

        let declared = self.types.object(object).fields.clone();
        let mut given: Vec<&str> = Vec::new();
        for field in fields {
            let found = self.check_expr(&field.value);
            match declared.iter().find(|(n, _)| *n == field.name.name) {
                Some((_, want)) => {
                    self.expect_type(*want, found, field.value.span, "this field");
                }
                None => self.error(
                    "E0348",
                    format!("`{}` has no field `{}`", name.name, field.name.name),
                    field.name.span,
                    "not a field of this type",
                ),
            }
            if given.contains(&field.name.name.as_str()) {
                self.error(
                    "E0349",
                    format!("field `{}` is given twice", field.name.name),
                    field.name.span,
                    "already set",
                );
            }
            given.push(&field.name.name);
        }

        // Every field is required: there is no optional type, so a missing one
        // would have to be filled with a value nobody chose.
        let missing: Vec<&str> = declared
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !given.contains(n))
            .collect();
        if !missing.is_empty() {
            self.error(
                "E0350",
                format!("`{}` is missing {}", name.name, join_names(&missing)),
                span,
                "every field has to be given",
            );
        }
        self.types.intern(Type::Object(object))
    }

    fn check_field_access(&mut self, base: &Expr, name: &Ident, span: Span) -> TypeId {
        let base_ty = self.check_expr(base);
        if self.types.is_error(base_ty) {
            return Types::ERROR;
        }
        // A field of a model's answer is still the model's answer. Losing the
        // label at the first field access would make the whole thing
        // decorative.
        let provenance = self.types.trust_of(base_ty).map(|(kind, _)| kind);
        let base_ty = self.types.untrusted(base_ty);
        let Some(object) = self.types.as_object(base_ty) else {
            let found = self.types.name(base_ty);
            self.error(
                "E0341",
                format!("{found} has no fields"),
                span,
                "only a record type has fields",
            );
            return Types::ERROR;
        };
        match self.types.object(object).field(&name.name) {
            Some((_, ty)) => match provenance {
                Some(kind) => self.types.trust(kind, ty),
                None => ty,
            },
            None => {
                let type_name = self.types.object(object).name.clone();
                self.error(
                    "E0341",
                    format!("`{type_name}` has no field `{}`", name.name),
                    name.span,
                    "not a field of this type",
                );
                Types::ERROR
            }
        }
    }

    /// An empty list literal in a position whose type is already written down.
    ///
    /// `[]` has no element type of its own, and `E0342` is right that guessing
    /// one would move the error to wherever the list is used. It is not right
    /// where the answer is beside it: a `let` annotation, a parameter, a
    /// return type. Asking again there is the checker declining to read what
    /// is in front of it.
    ///
    /// `Some(want)` when this took the position's type, `None` when the
    /// expression has to be checked on its own terms.
    fn empty_list_of(&mut self, e: &Expr, want: TypeId) -> Option<TypeId> {
        let empty = matches!(&e.kind, ExprKind::List { elements } if elements.is_empty());
        if empty && self.types.list_element(want).is_some() {
            self.node_types.insert(e.id, want);
            return Some(want);
        }
        None
    }

    fn check_list(&mut self, elements: &[Expr], span: Span) -> TypeId {
        let Some(first) = elements.first() else {
            // There is nothing to infer from, and guessing would make the
            // error appear wherever the list is used instead of here.
            self.error(
                "E0342",
                "an empty list needs a type annotation",
                span,
                "write `let xs: List<T> = [];`",
            );
            return Types::ERROR;
        };
        let element = self.check_expr(first);
        for other in &elements[1..] {
            let found = self.check_expr(other);
            self.expect_type(element, found, other.span, "this element");
        }
        if self.types.is_error(element) {
            return Types::ERROR;
        }
        self.types.list(element)
    }

    fn check_index(&mut self, base: &Expr, index: &Expr, span: Span) -> TypeId {
        let base_ty = self.check_expr(base);
        let index_ty = self.check_expr(index);
        let index_ty = self.types.untrusted(index_ty);
        self.expect_type(Types::INT, index_ty, index.span, "this index");
        if self.types.is_error(base_ty) {
            return Types::ERROR;
        }
        match self.element_type(base_ty) {
            Some(element) => element,
            None => {
                let found = self.types.name(self.types.untrusted(base_ty));
                self.error(
                    "E0351",
                    format!("{found} cannot be indexed"),
                    span,
                    "only a `List<T>` can be indexed",
                );
                Types::ERROR
            }
        }
    }

    /// Reports a value whose provenance makes it unusable here.
    ///
    /// The note used to point at `approve`, and it was a dead end: `approve`
    /// answers `HumanApproved<T>`, which this refuses in the same position for
    /// the same reason, so a program that followed the advice met the identical
    /// error one line later. What `approve` buys is reach (E0372), and that is
    /// where it is still offered. Here the way through is to ask a question
    /// about the value instead of computing one from it.
    fn reject_trust(&mut self, ty: TypeId, span: Span, what: &str) -> bool {
        let Some((_, inner)) = self.types.trust_of(ty) else {
            return false;
        };
        let is_bool = inner == Types::BOOL;
        let (outer, inner) = (self.types.name(ty), self.types.name(inner));
        self.error(
            "E0371",
            format!("{outer} cannot be used as {what}"),
            span,
            format!("this is where a {inner} came from, not a {inner}"),
        );
        if is_bool {
            // The one place a comparison is refused, and the reason it is: a
            // `Bool` compared with a literal is the `Bool` again.
            self.note(
                "comparing a Bool with a literal hands the Bool back, \
                 so this is the value rather than a question about it",
            );
        } else {
            self.note(
                "compare it instead, or ask `len`, `contains` or `starts_with` - \
                 those answer a question about a labelled value rather than hand one back",
            );
        }
        self.note(
            "`approve` changes which capabilities a value may reach, \
             not what may be computed from it",
        );
        true
    }

    fn finish(self) -> (Typed, Vec<Diagnostic>) {
        let entry = self.fn_ids.get("main").copied();
        let fns = self
            .fns
            .into_iter()
            .map(|f| FnInfo {
                name: f.name,
                params: f.params,
                ret: f.ret.unwrap_or(Types::UNIT),
                local_types: f.local_types,
                item_index: f.item_index,
                span: f.span,
            })
            .collect();
        (
            Typed {
                types: self.types,
                fns,
                node_types: self.node_types,
                res: self.res,
                caps: self.caps,
                agents: self.agents,
                entry,
            },
            self.diags,
        )
    }
}

/// Whether a pin is a digest.
///
/// `Digest::from_hex` rather than `Digest::parse`: the grant already says
/// `sha256`, so the string beside it is the number and nothing else.
fn is_sha256(text: &str) -> bool {
    sic_core::Digest::from_hex(text).is_some()
}

/// Records an error type for a node and returns it.
fn return_error(node_types: &mut HashMap<NodeId, TypeId>, node: NodeId) -> TypeId {
    node_types.insert(node, Types::ERROR);
    Types::ERROR
}

/// "`a`", "`a` and `b`", "`a`, `b` and `c`".
fn join_names(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        _ => format!(
            "{} and {}",
            quoted[..quoted.len() - 1].join(", "),
            quoted[quoted.len() - 1]
        ),
    }
}

/// Whether every path through a block ends in `return`.
///
/// Statements after a `return` are unreachable rather than wrong, so this only
/// asks whether a `return` is guaranteed, not where it sits.
fn always_returns(block: &Block) -> bool {
    block.stmts.iter().any(stmt_always_returns)
}

fn stmt_always_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::If(if_stmt) => {
            let then_returns = always_returns(&if_stmt.then_block);
            let else_returns = match if_stmt.else_branch.as_deref() {
                Some(ElseBranch::Block(b)) => always_returns(b),
                Some(ElseBranch::If(inner)) => stmt_always_returns(&Stmt::If(inner.clone())),
                None => false,
            };
            then_returns && else_returns
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
