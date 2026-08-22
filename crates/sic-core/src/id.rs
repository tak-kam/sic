//! Index newtypes used across the layers.
//!
//! Each is a thin wrapper around `u32`. Their only purpose is to make mixing
//! them up a type error; how they are allocated and what they mean is up to the
//! layer that owns them.

macro_rules! define_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.0)
            }
        }
    };
}

define_id!(
    /// Identifies an AST node. Types and other analysis results live in side
    /// tables keyed by this id, never in the AST itself.
    NodeId
);
define_id!(
    /// Index into the function table.
    FuncId
);
define_id!(
    /// A virtual register in the HIR.
    LocalId
);
define_id!(
    /// A basic block in the HIR.
    BlockId
);
define_id!(
    /// Index into the constant pool.
    ConstIdx
);
define_id!(
    /// Index into the capability manifest.
    CapId
);
define_id!(
    /// An interned type.
    TypeId
);
