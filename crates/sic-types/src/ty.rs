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
    Fn(FnSigId),
    /// Section 19 of the specification. Never constructed in v0.1; the variant
    /// exists so that adding it later does not reshape every match.
    Trust(TrustKind, TypeId),
    /// The result of an error. Using it produces no further diagnostics, which
    /// is what stops one mistake from cascading.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrustKind {
    Llm,
    Verified,
    HumanApproved,
    Observed,
    UserProvided,
    Secret,
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
            Type::Fn(sig) => {
                let s = self.sig(*sig);
                let params: Vec<String> = s.params.iter().map(|p| self.name(*p)).collect();
                format!("fn({}) -> {}", params.join(", "), self.name(s.ret))
            }
            Type::Trust(kind, inner) => format!("{kind:?}<{}>", self.name(*inner)),
            Type::Error => "<error>".into(),
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
