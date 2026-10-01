// src/ast/mod.rs
//! Abstract Syntax Tree for Ubel Stratum.
//!
//! All nodes are arena-allocated via `AstArena` (backed by `bumpalo`).
//! Every node type carries a `'ast` lifetime tied to the arena that
//! owns it.  Because all fields are either primitive types, fat pointers
//! (`&'ast [T]`), or thin pointers (`&'ast T`), every AST type is `Copy`.
//!
//! # Module layout
//!
//! | Module          | Contents                                              |
//! |-----------------|-------------------------------------------------------|
//! | `arena`         | `AstArena` — the bumpalo arena wrapper                |
//! | `common`        | `Span`, `Ident`, `Visibility`, operators, attributes  |
//! | `literals`      | `Literal`, `InterpolationPart`                        |
//! | `types`         | `Type`, `TypeKind`, `FunctionType`                    |
//! | `patterns`      | `Pattern`, `DestructurePattern`, field patterns       |
//! | `expressions`   | `Expr`, lambdas, match, LINQ, if-expressions          |
//! | `statements`    | `Stmt`, `Block`, allocator kinds, using/defer         |
//! | `declarations`  | Functions, structs, enums, traits, impls, extends     |
//! | `root`          | `Program`, `Item`, package/import declarations        |

#![allow(dead_code)]

pub mod arena;
pub mod common;
pub mod literals;
pub mod types;
pub mod patterns;
pub mod expressions;
pub mod statements;
pub mod declarations;
pub mod root;
pub mod visitor;

// ── Flat re-exports ───────────────────────────────────────────────

pub use arena::{AstArena, BumpVec};

pub use common::{
    AssignOp, AttrArg, AttrValue, Attribute,
    BinOp, GenericParam,
    Ident, LifetimeConstraint, LifetimeParam,
    QualifiedIdent, Span, TierAnnotation, UnaryOp, Visibility,
};

pub use literals::{InterpolationPart, Literal};

pub use types::{FunctionType, Type, TypeKind};

pub use patterns::{
    ArrayDestructure, DestructureElement, DestructurePattern,
    EnumPatternPayload, FieldDestructure, FieldPattern,
    Pattern, PatternKind, StructDestructure, TupleDestructure,
};

pub use expressions::{
    Arg, ArgKind, DictEntry, ElifBranch, Expr, ExprKind,
    FieldInit, IfBranchBody, IfExpr, Lambda, LambdaBody, LambdaParam,
    MatchArm, MatchArmBody, MatchExpr,
    ObjectField, OptionalAccess, OrElseFallback,
};

pub use statements::{
    AllocatorKind, BindingTarget, Block, SizeExpr, SizeUnit,
    Stmt, StmtKind, UsingBinding,
};

pub use declarations::{
    ConstDecl, EnumDecl, EnumVariant, EnumVariantPayload,
    ExtendDecl, FieldDecl, FunctionDecl, ImplBlock,
    MethodDecl, MethodSig, Param, ParamKind, PropertyDecl,
    ReturnType, StaticDecl, StructDecl, StructMember, TraitDecl, TraitItem,
    TypeAlias,
};

pub use root::{Import, ImportItems, ImportKind, Item, PackageDecl, Program};
