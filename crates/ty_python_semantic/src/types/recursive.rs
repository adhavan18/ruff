//! Core representation for recursive types.

use salsa::plumbing::AsId;

use crate::Db;
use crate::types::{Type, TypeAliasType};

/// Identifier for the bound variable of a recursive type.
///
/// A recursive type is represented as `mu binder. body`, where occurrences of
/// `Type::Divergent` carrying this binder identify recursive references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub struct BinderId(salsa::Id);

// `salsa::Id` is an index into Salsa storage, whose memory is tracked separately.
impl get_size2::GetSize for BinderId {}

impl BinderId {
    pub(crate) const fn new(id: salsa::Id) -> Self {
        Self(id)
    }

    pub(crate) const fn into_id(self) -> salsa::Id {
        self.0
    }
}

/// Source of a recursive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update, get_size2::GetSize)]
pub enum RecursiveOrigin<'db> {
    /// A structural recursion not directly tied to a named alias.
    Implicit,
    /// Recursion introduced while resolving a type alias.
    TypeAlias(TypeAliasType<'db>),
}

impl<'db> RecursiveOrigin<'db> {
    #[expect(dead_code, reason = "staged API for recursive type construction")]
    pub(crate) fn source_type(self) -> Option<Type<'db>> {
        match self {
            Self::Implicit => None,
            Self::TypeAlias(alias) => Some(Type::TypeAlias(alias)),
        }
    }

    pub(crate) fn matches_type(self, db: &'db dyn Db, ty: Type<'db>) -> bool {
        match (self, ty) {
            (Self::Implicit, _) => false,
            (Self::TypeAlias(alias), Type::TypeAlias(other)) => {
                alias.definition(db) == other.definition(db)
            }
            _ => false,
        }
    }

    #[expect(dead_code, reason = "staged API for recursive type construction")]
    pub(crate) fn contains_in_type(self, db: &'db dyn Db, ty: Type<'db>) -> bool {
        crate::types::visitor::any_over_type(db, ty, true, |inner| self.matches_type(db, inner))
    }

    #[expect(dead_code, reason = "staged API for recursive type construction")]
    pub(crate) fn binder_id(self, db: &'db dyn Db) -> Option<salsa::Id> {
        match self {
            Self::Implicit => None,
            Self::TypeAlias(TypeAliasType::PEP695(alias)) => Some(alias.as_id()),
            Self::TypeAlias(TypeAliasType::ManualPEP695(alias)) => {
                Some(alias.definition(db).as_id())
            }
        }
    }
}

/// A recursive type `mu binder. body`.
#[salsa::interned(debug, heap_size=ruff_memory_usage::heap_size)]
pub struct RecursiveType<'db> {
    pub binder: BinderId,
    pub origin: RecursiveOrigin<'db>,
    pub body: Type<'db>,
}

// The Salsa heap is tracked separately.
impl get_size2::GetSize for RecursiveType<'_> {}

#[salsa::tracked]
impl<'db> RecursiveType<'db> {
    pub(crate) fn build(
        db: &'db dyn Db,
        binder_id: salsa::Id,
        origin: RecursiveOrigin<'db>,
        body: Type<'db>,
    ) -> Type<'db> {
        Type::Recursive(Self::new(db, BinderId::new(binder_id), origin, body))
    }

    pub(crate) fn binder_id(self, db: &'db dyn Db) -> salsa::Id {
        self.binder(db).into_id()
    }
}

pub(super) fn walk_recursive_type<'db, V: crate::types::visitor::TypeVisitor<'db> + ?Sized>(
    db: &'db dyn Db,
    recursive: RecursiveType<'db>,
    visitor: &V,
) {
    visitor.visit_type(db, recursive.body(db));
}
