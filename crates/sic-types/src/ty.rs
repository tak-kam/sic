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
}

impl TrustKind {
    pub fn name(self) -> &'static str {
        match self {
            TrustKind::Llm => "LLM",
            TrustKind::HumanApproved => "HumanApproved",
        }
    }

    pub fn from_name(name: &str) -> Option<TrustKind> {
        match name {
            "LLM" => Some(TrustKind::Llm),
            "HumanApproved" => Some(TrustKind::HumanApproved),
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

    pub fn object_count(&self) -> usize {
        self.objects.len()
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
            _ => None,
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

    #[test]
    fn interning_is_stable() {
        let mut t = Types::new();
        let a = t.intern(Type::List(Types::INT));
        let b = t.intern(Type::List(Types::INT));
        assert_eq!(a, b);
        assert_eq!(t.name(a), "List<Int>");
    }
}
