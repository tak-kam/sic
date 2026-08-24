//! Type checking and name resolution.
//!
//! Functions are checked in declaration order. A function without a return type
//! annotation gets its type from the `return` statements in its body, so calling
//! it before it has been checked requires an annotation (E0306). That rule keeps
//! inference to a single forward pass with no constraint solving.

use std::collections::{HashMap, HashSet};

use sic_core::{AgentId, CapId, Diagnostic, FuncId, Label, LocalId, NodeId, Span, TypeId};
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
            let id = self.types.declare_object(decl.name.name.clone());
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
                let id = CapId(self.caps.len() as u32);
                self.cap_ids.insert(full.clone(), id);
                self.caps.push(CapEntry {
                    name: full,
                    kind: sig.kind,
                    constraint,
                    pin,
                    args,
                    repeatable: grant.repeatable,
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
                // An empty list has no element type of its own, so an
                // annotation is the only thing that can give it one.
                let annotated = ty.as_ref().map(|t| self.resolve_type(t));
                let empty_list =
                    matches!(&init.kind, ExprKind::List { elements } if elements.is_empty());
                let init_ty = match (annotated, empty_list) {
                    (Some(want), true) if self.types.list_element(want).is_some() => {
                        self.node_types.insert(init.id, want);
                        want
                    }
                    _ => {
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
                    Some(e) => self.check_expr(e),
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
            Stmt::Expr { expr, .. } => {
                self.check_expr(expr);
            }
        }
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
        // Arithmetic on a value whose provenance matters is exactly where
        // provenance gets lost.
        if self.reject_trust(l, lhs.span, "an operand")
            || self.reject_trust(r, rhs.span, "an operand")
        {
            return Types::ERROR;
        }
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
            BinOp::Eq | BinOp::Ne => l == Types::INT || l == Types::BOOL,
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
            if l == Types::FLOAT || l == Types::STR {
                self.note("v0.1 supports arithmetic and comparison on Int only");
            }
            return Types::ERROR;
        }

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => l,
            _ => Types::BOOL,
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
        if let Some(policy_span) = policy.span {
            self.error(
                "E0330",
                "`retry` and `timeout` apply to capability calls only",
                policy_span,
                "this is a function call",
            );
            self.note("a function has no effect to retry or to wait for");
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
            // `len` is the only built-in function. It is looked up last, so a
            // module that defines its own `len` gets that one.
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
            let found = self.check_expr(arg);
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
            let found = self.check_expr(arg);
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
        self.types.trust(TrustKind::HumanApproved, value)
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
        let options = self.check_expr(&args[1]);
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
        self.expect_type(Types::STR, text, args[0].span, "this document");

        match self.json_target {
            Some(ty) => ty,
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
        // An element of a model's answer is still the model's answer, the same
        // way a field is.
        let provenance = self.types.trust_of(base_ty).map(|(kind, _)| kind);
        let base_ty = self.types.untrusted(base_ty);
        match self.types.list_element(base_ty) {
            Some(element) => match provenance {
                Some(kind) => self.types.trust(kind, element),
                None => element,
            },
            None => {
                let found = self.types.name(base_ty);
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
    fn reject_trust(&mut self, ty: TypeId, span: Span, what: &str) -> bool {
        let Some((kind, inner)) = self.types.trust_of(ty) else {
            return false;
        };
        let (outer, inner) = (self.types.name(ty), self.types.name(inner));
        self.error(
            "E0371",
            format!("{outer} cannot be used as {what}"),
            span,
            format!("this is where a {inner} came from, not a {inner}"),
        );
        if kind == TrustKind::Llm {
            self.note(
                "`approve(question, value)` turns a model's answer into one a person signed off",
            );
        }
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

/// Whether a string is 64 hexadecimal characters.
fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
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
