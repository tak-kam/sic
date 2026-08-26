//! Type representation.
//!
//! Types are interned, so comparing two of them compares two integers. The
//! primitives get fixed ids, which lets the rest of the compiler name them
//! without a lookup.

use std::collections::HashMap;

use sic_core::TypeId;

/// Index into the function signature table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnSigId(pub u32);

/// Index into the record type table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub u32);

impl ObjectId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A user-defined record type.
///
/// Fields are ordered: the source addresses them by name, the bytecode by
/// position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDef {
    pub name: String,
    pub fields: Vec<(String, TypeId)>,
}

impl ObjectDef {
    pub fn field(&self, name: &str) -> Option<(usize, TypeId)> {
        self.fields
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| (i, self.fields[i].1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Unit,
    Bool,
    /// i64
    Int,
    /// f64
    Float,
    Str,
    List(TypeId),
    /// A running computation and, eventually, what it produces.
    Task(TypeId),
    /// A user-defined record.
    Object(ObjectId),
    Fn(FnSigId),
    /// Where a value came from. See docs/design/trust.md.
    ///
    /// This is a compile-time distinction only: trust is erased before the
    /// bytecode, because the rule being enforced is "this program may not be
    /// written", which is a claim about the program rather than about a run.
    Trust(TrustKind, TypeId),
    /// The result of an error. Using it produces no further diagnostics, which
    /// is what stops one mistake from cascading.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrustKind {
    /// A model produced it.
    Llm,
    /// A person approved it.
    HumanApproved,
    /// A program printed it. Not verified, not approved, and not written by
    /// whoever wrote the program that read it.
    Observed,
    /// A person picked it out of a list the program itself wrote.
    HumanChosen,
}

impl TrustKind {
    pub fn name(self) -> &'static str {
        match self {
            TrustKind::Llm => "LLM",
            TrustKind::HumanApproved => "HumanApproved",
            TrustKind::Observed => "Observed",
            TrustKind::HumanChosen => "HumanChosen",
        }
    }

    pub fn from_name(name: &str) -> Option<TrustKind> {
        match name {
            "LLM" => Some(TrustKind::Llm),
            "HumanApproved" => Some(TrustKind::HumanApproved),
            "Observed" => Some(TrustKind::Observed),
            "HumanChosen" => Some(TrustKind::HumanChosen),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSig {
    pub params: Vec<TypeId>,
    pub ret: TypeId,
}

/// The interner. Ids 0 to 5 are always the primitives below.
#[derive(Debug, Clone)]
pub struct Types {
    types: Vec<Type>,
    index: HashMap<Type, TypeId>,
    sigs: Vec<FnSig>,
    objects: Vec<ObjectDef>,
}

impl Types {
    pub const UNIT: TypeId = TypeId(0);
    pub const BOOL: TypeId = TypeId(1);
    pub const INT: TypeId = TypeId(2);
    pub const FLOAT: TypeId = TypeId(3);
    pub const STR: TypeId = TypeId(4);
    pub const ERROR: TypeId = TypeId(5);
    /// An argument vector. Interned here so that a builtin capability
    /// signature, which is a `const`, can name it.
    pub const LIST_STR: TypeId = TypeId(6);
    /// What a program printed. Interned here for the same reason.
    pub const OBSERVED_STR: TypeId = TypeId(7);
    /// What a program did: `{ code: Int, output: Observed<String> }`.
    ///
    /// The only record type the language declares for itself, and it exists
    /// because `process.run` has two facts to return and no capability can
    /// return two values. See `docs/design/output.md` §9.
    ///
    /// It is not wrapped in a provenance. Wrapping the record would make
    /// `code` an `Observed<Int>`, and a trusted value cannot be an operand, so
    /// `if r.code == 0` would not compile - which is the whole reason the type
    /// exists. The provenance belongs to the field that has one: the text a
    /// program printed, exactly as `process.capture` returns it.
    pub const EXIT: TypeId = TypeId(8);
    /// What a program printed, one line at a time. Interned for the same
    /// reason `LIST_STR` is: a builtin capability signature is a `const`.
    pub const OBSERVED_LIST_STR: TypeId = TypeId(9);

    pub fn new() -> Self {
        let mut t = Self {
            types: Vec::new(),
            index: HashMap::new(),
            sigs: Vec::new(),
            objects: Vec::new(),
        };
        // The order here defines the constants above.
        assert_eq!(t.intern(Type::Unit), Self::UNIT);
        assert_eq!(t.intern(Type::Bool), Self::BOOL);
        assert_eq!(t.intern(Type::Int), Self::INT);
        assert_eq!(t.intern(Type::Float), Self::FLOAT);
        assert_eq!(t.intern(Type::Str), Self::STR);
        assert_eq!(t.intern(Type::Error), Self::ERROR);
        assert_eq!(t.intern(Type::List(Self::STR)), Self::LIST_STR);
        assert_eq!(
            t.intern(Type::Trust(TrustKind::Observed, Self::STR)),
            Self::OBSERVED_STR
        );
        // The one record the language declares. Its fields are set here rather
        // than resolved from source, so object 0 is always this and a module's
        // own types start at 1.
        let exit = t.declare_object("Exit");
        t.set_object_fields(
            exit,
            vec![
                ("code".to_string(), Self::INT),
                ("output".to_string(), Self::OBSERVED_STR),
            ],
        );
        assert_eq!(t.intern(Type::Object(exit)), Self::EXIT);
        assert_eq!(
            t.intern(Type::Trust(TrustKind::Observed, Self::LIST_STR)),
            Self::OBSERVED_LIST_STR
        );
        t
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self.index.get(&ty) {
            return *id;
        }
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty.clone());
        self.index.insert(ty, id);
        id
    }

    pub fn get(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
    }

    pub fn add_sig(&mut self, sig: FnSig) -> FnSigId {
        let id = FnSigId(self.sigs.len() as u32);
        self.sigs.push(sig);
        id
    }

    pub fn sig(&self, id: FnSigId) -> &FnSig {
        &self.sigs[id.0 as usize]
    }

    /// Declares a record type, before its fields are known.
    ///
    /// Two types may refer to each other's names, so the id has to exist before
    /// either body is resolved.
    pub fn declare_object(&mut self, name: impl Into<String>) -> ObjectId {
        let id = ObjectId(self.objects.len() as u32);
        self.objects.push(ObjectDef {
            name: name.into(),
            fields: Vec::new(),
        });
        id
    }

    pub fn set_object_fields(&mut self, id: ObjectId, fields: Vec<(String, TypeId)>) {
        self.objects[id.index()].fields = fields;
    }

    pub fn object(&self, id: ObjectId) -> &ObjectDef {
        &self.objects[id.index()]
    }

    /// The record a type is, if it is one.
    pub fn as_object(&self, id: TypeId) -> Option<ObjectId> {
        match self.get(id) {
            Type::Object(object) => Some(*object),
            _ => None,
        }
    }

    /// Wraps a type in a provenance.
    ///
    /// Wrapping something already wrapped replaces the provenance rather than
    /// nesting: a value has one origin, and `LLM<HumanApproved<T>>` would say
    /// nothing useful.
    pub fn trust(&mut self, kind: TrustKind, inner: TypeId) -> TypeId {
        let inner = self.untrusted(inner);
        self.intern(Type::Trust(kind, inner))
    }

    /// The provenance of a type, if it has one.
    pub fn trust_of(&self, id: TypeId) -> Option<(TrustKind, TypeId)> {
        match self.get(id) {
            Type::Trust(kind, inner) => Some((*kind, *inner)),
            // A vector of values a program printed is a value a program
            // printed. Without this, `["checkout", sha]` would be a way past
            // the rule that stops `sha` from deciding what runs.
            Type::List(element) => self.trust_of(*element),
            _ => None,
        }
    }

    /// The type without any provenance, at any depth.
    ///
    /// A capability erases trust, so `List<Observed<String>>` satisfies a
    /// `List<String>` parameter - once the rule about where that value may go
    /// has already been applied. `trust_of` looks through a list for the same
    /// reason.
    pub fn untrusted_deep(&mut self, id: TypeId) -> TypeId {
        match *self.get(id) {
            Type::Trust(_, inner) => self.untrusted_deep(inner),
            Type::List(element) => {
                let element = self.untrusted_deep(element);
                self.list(element)
            }
            _ => id,
        }
    }

    /// The type without its provenance.
    pub fn untrusted(&self, id: TypeId) -> TypeId {
        match self.get(id) {
            Type::Trust(_, inner) => *inner,
            _ => id,
        }
    }

    /// What a list holds, if this is a list type.
    pub fn list_element(&self, id: TypeId) -> Option<TypeId> {
        match self.get(id) {
            Type::List(inner) => Some(*inner),
            _ => None,
        }
    }

    pub fn list(&mut self, element: TypeId) -> TypeId {
        self.intern(Type::List(element))
    }

    /// Resolves a type name as written in the source.
    ///
    /// Only the primitive names exist in v0.1; user-defined types arrive with
    /// `type` declarations in a later phase.
    pub fn by_name(&self, name: &str) -> Option<TypeId> {
        Some(match name {
            "Unit" => Self::UNIT,
            "Bool" => Self::BOOL,
            "Int" => Self::INT,
            "Float" => Self::FLOAT,
            "String" => Self::STR,
            // Naming it here is also what makes `type Exit { .. }` an E0345:
            // a module may not redefine a type the language declares.
            "Exit" => Self::EXIT,
            _ => return None,
        })
    }

    /// How a type is spelled in a diagnostic.
    pub fn name(&self, id: TypeId) -> String {
        match self.get(id) {
            Type::Unit => "Unit".into(),
            Type::Bool => "Bool".into(),
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::Str => "String".into(),
            Type::List(inner) => format!("List<{}>", self.name(*inner)),
            Type::Task(inner) => format!("Task<{}>", self.name(*inner)),
            Type::Fn(sig) => {
                let s = self.sig(*sig);
                let params: Vec<String> = s.params.iter().map(|p| self.name(*p)).collect();
                format!("fn({}) -> {}", params.join(", "), self.name(s.ret))
            }
            Type::Object(object) => self.object(*object).name.clone(),
            Type::Trust(kind, inner) => format!("{}<{}>", kind.name(), self.name(*inner)),
            Type::Error => "<error>".into(),
        }
    }

    /// How a type is spelled to whatever has to produce a value of it.
    ///
    /// An `agent` declares its output type once, and that declaration is the
    /// only place the shape of the answer is written down. Something being
    /// asked for one has to be told, or it answers with prose and the run fails
    /// at the validation - correctly, and for the wrong reason. So the shape
    /// travels with the prompt: see `docs/design/driving.md`.
    ///
    /// It is a sketch rather than JSON Schema. A schema document would be
    /// larger than the prompt it decorates, and the thing reading it is not a
    /// validator - the validator is `FROM_JSON`, and it already exists.
    pub fn shape(&self, id: TypeId) -> String {
        self.shape_at(id, &mut Vec::new())
    }

    /// `open` holds the records being described further up, so a type that
    /// contains a list of itself names itself instead of recurring forever.
    fn shape_at(&self, id: TypeId, open: &mut Vec<ObjectId>) -> String {
        match self.get(id) {
            Type::Unit => "null".into(),
            Type::Bool => "boolean".into(),
            Type::Int => "integer".into(),
            Type::Float => "number".into(),
            Type::Str => "string".into(),
            Type::List(inner) => format!("[{}]", self.shape_at(*inner, open)),
            // Trust is a claim about where a value came from, which is not
            // something whoever answers can produce or would know what to do
            // with.
            Type::Trust(_, inner) => self.shape_at(*inner, open),
            Type::Object(object) => {
                let def = self.object(*object);
                if open.contains(object) {
                    return def.name.clone();
                }
                open.push(*object);
                let fields: Vec<String> = def
                    .fields
                    .iter()
                    .map(|(name, ty)| format!("{name:?}: {}", self.shape_at(*ty, open)))
                    .collect();
                open.pop();
                format!("{{{}}}", fields.join(", "))
            }
            // Neither can cross a capability boundary, so neither can be asked
            // for. Naming the type is more use than an empty string.
            Type::Task(_) | Type::Fn(_) | Type::Error => self.name(id),
        }
    }

    /// The type of a task producing `inner`.
    pub fn task(&mut self, inner: TypeId) -> TypeId {
        self.intern(Type::Task(inner))
    }

    /// What a task produces, if this is a task type.
    pub fn task_output(&self, id: TypeId) -> Option<TypeId> {
        match self.get(id) {
            Type::Task(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Whether a diagnostic should be suppressed because the type already
    /// stands for a reported error.
    pub fn is_error(&self, id: TypeId) -> bool {
        id == Self::ERROR
    }
}

impl Default for Types {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_have_fixed_ids() {
        let t = Types::new();
        assert_eq!(t.get(Types::INT), &Type::Int);
        assert_eq!(t.by_name("String"), Some(Types::STR));
        assert_eq!(t.by_name("Nope"), None);
    }

    /// The shape is what whoever answers is told, so it has to say what JSON
    /// they should write, not what sic calls the type.
    #[test]
    fn a_shape_is_the_json_that_would_fit() {
        let mut t = Types::new();
        assert_eq!(t.shape(Types::STR), "string");
        assert_eq!(t.shape(Types::INT), "integer");
        assert_eq!(t.shape(Types::FLOAT), "number");
        assert_eq!(t.shape(Types::BOOL), "boolean");

        let strings = t.intern(Type::List(Types::STR));
        assert_eq!(t.shape(strings), "[string]");

        let ticket = t.declare_object("Ticket");
        t.set_object_fields(
            ticket,
            vec![
                ("title".into(), Types::STR),
                ("severity".into(), Types::INT),
            ],
        );
        let ticket_ty = t.intern(Type::Object(ticket));
        assert_eq!(
            t.shape(ticket_ty),
            "{\"title\": string, \"severity\": integer}"
        );

        // Trust says where a value came from, which is not something whoever
        // answers can produce.
        let trusted = t.trust(TrustKind::Llm, ticket_ty);
        assert_eq!(t.shape(trusted), t.shape(ticket_ty));
    }

    /// A record may hold a list of itself, so the renderer has to stop.
    #[test]
    fn a_type_containing_itself_names_itself() {
        let mut t = Types::new();
        let node = t.declare_object("Node");
        let node_ty = t.intern(Type::Object(node));
        let children = t.intern(Type::List(node_ty));
        t.set_object_fields(
            node,
            vec![("name".into(), Types::STR), ("children".into(), children)],
        );
        assert_eq!(t.shape(node_ty), "{\"name\": string, \"children\": [Node]}");
    }

    #[test]
    fn interning_is_stable() {
        let mut t = Types::new();
        let a = t.intern(Type::List(Types::INT));
        let b = t.intern(Type::List(Types::INT));
        assert_eq!(a, b);
        assert_eq!(t.name(a), "List<Int>");
    }
}
