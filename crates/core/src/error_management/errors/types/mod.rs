// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "error_management/errors/types/mod.rs"
// ============================================================================
// src/error_management/errors/types/mod.rs
//! Errors produced during ordinary type inference and type checking.
//!
//! Tier and arena enforcement errors used to live in this same enum
//! (as the TYPE-2xx range) but were physically split out into
//! `TierError` (see `../tier/mod.rs`) — see docs/DIAGNOSTICS_RULES.md
//! §9. This file now only carries TYPE-1xx.

use crate::lexer::Span;
use std::fmt;

/// Every error that can be raised during ordinary type checking
/// (not tier/arena enforcement — see `TierError` for that).
#[derive(Debug, Clone)]
pub enum TypeError {
    // ── Type mismatch ─────────────────────────────────────────────
    /// The types of two expressions that must agree do not.
    TypeMismatch {
        expected:     String,   // human-readable type name
        found:        String,
        span:         Span,
        /// Where the expected type was established, if known.
        because_of:   Option<Span>,
    },

    /// A function was called with the wrong number of arguments.
    ArgumentCountMismatch {
        expected: usize,
        found:    usize,
        span:     Span,
    },

    /// Tried to access a field that does not exist on a type.
    NoSuchField {
        field:   String,
        on_type: String,
        span:    Span,
    },

    /// Tried to call a method that does not exist on a type.
    NoSuchMethod {
        method:  String,
        on_type: String,
        span:    Span,
    },

    /// `?` operator used on a non-fallible type (not `T!`).
    TryOnNonFallible {
        found: String,
        span:  Span,
    },

    /// `await` used on a non-`Task<T>` type.
    AwaitOnNonTask {
        found: String,
        span:  Span,
    },

    /// `*`/`deref` used on a non-reference type.
    DerefOnNonReference {
        found: String,
        span:  Span,
    },

    /// A `{value:spec}` format spec used a part that doesn't apply to
    /// `value`'s type — currently only `.precision` is type-restricted
    /// (Float/Double/Str); width/align/`?` apply to anything.
    InvalidFormatSpec {
        spec_part: String,
        on_type:   String,
        span:      Span,
    },

    /// `#` (the alternate form) with no trailing numeric base (`x`/`X`/
    /// `o`/`b`) in the same format spec. Distinct from
    /// `InvalidFormatSpec`: this isn't about `value`'s type at all (a
    /// `#` on an `Int` with no base is just as invalid as one on a
    /// `Str`), it's about a spec-internal combination that's missing its
    /// other half, the same shape `DeriveRequiresOther` covers for
    /// `@derive(...)`.
    AlternateFormatWithoutBase {
        span: Span,
    },

    /// `@derive(X)` named something that isn't a recognized, supported
    /// derive trait *in this context*. Covers three distinct real cases
    /// with one variant: a genuinely unknown name; `Debug`/`Display`
    /// anywhere (both automatic for every struct/enum already, no
    /// opt-in needed — docs/PRINT_FORMAT_RULES.md §6); and `PartialEq`
    /// specifically on an `enum` (an enum's `==` is already structural,
    /// see `Value::equals`'s Enum arm — only a `struct`'s default needs
    /// flipping). `trait_name` holds a readable description of the bad
    /// argument, not necessarily a real identifier — a non-`Ident` arg
    /// (`@derive("PartialEq")`, `@derive(x = 1)`) reaches here too, not
    /// just misspelled/inapplicable names.
    UnknownDeriveTrait {
        trait_name: String,
        span:       Span,
    },

    /// `@derive(X)` named a real, recognized trait, but without a
    /// companion trait it depends on also being present in the same
    /// `@derive(...)` list. `Eq` needs `PartialEq`, `PartialOrd` needs
    /// `PartialEq`, `Ord` needs `PartialOrd` (which transitively covers
    /// `PartialEq` too, but both are checked directly rather than
    /// relying on transitivity, so the message always names the
    /// *immediate* gap), `Hash` needs `Eq` (not a real Rust supertrait
    /// bound for `Hash` specifically, but the bound every actual
    /// hash-map API uses in practice, `K: Eq + Hash`), and the entire
    /// reason this project wants `Hash` at all (a future `Dict` key).
    /// `Clone` has no prerequisite. Distinct from `UnknownDeriveTrait`
    /// on purpose: this isn't an invalid or unrecognized name, it's a
    /// valid one requested incompletely.
    DeriveRequiresOther {
        trait_name: String,
        requires:   String,
        span:       Span,
    },

    /// `<`/`<=`/`>`/`>=` used on a type with no ordering: anything other
    /// than `Int`/`Float`/`Double`/`Str`/`Bool`, or a `struct` that
    /// hasn't `@derive`d `PartialOrd`/`Ord`. Previously a silent gap:
    /// these operators only ever worked for numeric operands
    /// (`eval_binop`'s `promote_numeric`), and anything else reached a
    /// runtime panic (`"arithmetic not supported on {type}"`) with no
    /// sema-time check at all. `on_type` is the resolved type as
    /// displayed to the person, not necessarily a struct. This also
    /// catches e.g. two `List`s compared with `<`, which never had an
    /// ordering to begin with and isn't expected to gain one here.
    TypeNotOrderable {
        on_type: String,
        span:    Span,
    },

    /// A suffixed integer literal's value doesn't fit the suffix's
    /// declared width (`300u8`), or an unsuffixed literal auto-promoted
    /// to `u64` still overflowed even that (unreachable today -- the
    /// lexer's own `u64::MAX` ceiling already catches that case as a
    /// lex error first -- kept here anyway so this check is complete on
    /// its own terms, not dependent on the lexer's ceiling matching it).
    /// Caught at sema time rather than silently wrapping, matching
    /// Rust's own `error: literal out of range for` on the same source.
    ///
    /// Also raised for an *unsuffixed* literal whose inferred type turned
    /// out to be too narrow for it (`let x: u8 = 300`, `f(-1)` where the
    /// parameter is `u32`). Then `inferred` is `true`, `suffix` holds the
    /// name of the type the context picked, and `negative` says whether
    /// the literal sat directly under a unary `-` (`raw` is always the
    /// magnitude).
    IntLiteralOutOfRange {
        suffix:   &'static str,
        raw:      u64,
        negative: bool,
        inferred: bool,
        span:     Span,
    },

    /// A type could not be inferred — too ambiguous.
    CannotInferType {
        span:       Span,
        suggestion: Option<String>,
    },

    /// A generic was instantiated with the wrong number of type arguments.
    GenericArgCountMismatch {
        type_name: String,
        expected:  usize,
        found:     usize,
        span:      Span,
    },

    // ── Enum sema (ENUM_RULES.md) ────────────────────────────────

    /// `EnumName.variant_name` where `variant_name` isn't a real variant
    /// of `EnumName` — either as an expression (`Direction.Northeast`) or
    /// a pattern (`Direction.Northeast => ...`).
    UnknownVariant {
        enum_name:    String,
        variant_name: String,
        span:         Span,
    },

    /// A tuple-payload variant constructed or matched with the wrong
    /// number of elements — `Result.Ok(1, 2)` when `Ok` holds exactly one.
    VariantArityMismatch {
        enum_name:    String,
        variant_name: String,
        expected:     usize,
        found:        usize,
        span:         Span,
    },

    /// A `match` over an enum scrutinee doesn't cover every variant and
    /// has no wildcard/catch-all arm.
    NonExhaustiveMatch {
        missing_variants: Vec<String>,
        span:             Span,
    },

    /// An enum declares both an explicit `Discriminant` variant and a
    /// payload-carrying (`Tuple`/`Struct`) variant — ENUM_RULES.md §4,
    /// item 2: disallowed, since there's no defined runtime
    /// representation for "this variant has both a chosen ordinal and a
    /// payload shape."
    MixedDiscriminantAndPayload {
        enum_name: String,
        span:      Span,
    },

    // ── InlineList (DATASTRUCTURES.md §5) ────────────────────────

    /// `InlineList.new(capacity)` — `capacity` must be a literal
    /// integer, checked directly against the argument's own AST node
    /// (not a general const-expression evaluator — genuine inline/stack
    /// storage needs its size known at compile time, and a literal is
    /// the narrowest thing that guarantees that without building real
    /// const generics, which the language has nowhere else either).
    InlineListCapacityNotLiteral {
        span: Span,
    },
}

impl TypeError {
    pub fn span(&self) -> Span {
        match self {
            TypeError::TypeMismatch               { span, .. } => *span,
            TypeError::ArgumentCountMismatch      { span, .. } => *span,
            TypeError::NoSuchField                { span, .. } => *span,
            TypeError::NoSuchMethod               { span, .. } => *span,
            TypeError::TryOnNonFallible           { span, .. } => *span,
            TypeError::AwaitOnNonTask             { span, .. } => *span,
            TypeError::DerefOnNonReference        { span, .. } => *span,
            TypeError::InvalidFormatSpec          { span, .. } => *span,
            TypeError::AlternateFormatWithoutBase { span, .. } => *span,
            TypeError::UnknownDeriveTrait          { span, .. } => *span,
            TypeError::DeriveRequiresOther          { span, .. } => *span,
            TypeError::TypeNotOrderable             { span, .. } => *span,
            TypeError::IntLiteralOutOfRange        { span, .. } => *span,
            TypeError::CannotInferType            { span, .. } => *span,
            TypeError::GenericArgCountMismatch    { span, .. } => *span,
            TypeError::UnknownVariant             { span, .. } => *span,
            TypeError::VariantArityMismatch       { span, .. } => *span,
            TypeError::NonExhaustiveMatch         { span, .. } => *span,
            TypeError::MixedDiscriminantAndPayload{ span, .. } => *span,
            TypeError::InlineListCapacityNotLiteral{ span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            TypeError::TypeMismatch { expected, found, .. } =>
                format!("type mismatch: expected `{}`, found `{}`", expected, found),

            TypeError::ArgumentCountMismatch { expected, found, .. } =>
                format!("expected {} argument(s), found {}", expected, found),

            TypeError::NoSuchField { field, on_type, .. } =>
                format!("type `{}` has no field `{}`", on_type, field),

            TypeError::NoSuchMethod { method, on_type, .. } =>
                format!("type `{}` has no method `{}`", on_type, method),

            TypeError::TryOnNonFallible { found, .. } =>
                format!("`?` requires a fallible type (`T!`), found `{}`", found),

            TypeError::AwaitOnNonTask { found, .. } =>
                format!("`await` requires `Task<T>`, found `{}`", found),

            TypeError::DerefOnNonReference { found, .. } =>
                format!("`*`/`deref` requires a reference type (`&T`/`ref T`), found `{}`", found),

            TypeError::InvalidFormatSpec { spec_part, on_type, .. } =>
                format!("`{}` in a format spec doesn't apply to type `{}`", spec_part, on_type),

            TypeError::AlternateFormatWithoutBase { .. } =>
                "`#` in a format spec needs a numeric base (`x`/`X`/`o`/`b`)".to_string(),

            TypeError::UnknownDeriveTrait { trait_name, .. }
                if trait_name == "Debug" || trait_name == "Display" || trait_name == "PartialEq" =>
                format!("`@derive({})` is unnecessary — this is already automatic", trait_name),

            TypeError::UnknownDeriveTrait { trait_name, .. } =>
                format!("unknown derive trait `{}`", trait_name),

            TypeError::DeriveRequiresOther { trait_name, requires, .. } =>
                format!("`@derive({})` also needs `@derive({})`", trait_name, requires),

            TypeError::TypeNotOrderable { on_type, .. } =>
                format!("type `{}` doesn't support ordering comparisons", on_type),

            TypeError::IntLiteralOutOfRange { suffix, raw, negative, inferred, .. } => {
                let sign = if *negative { "-" } else { "" };
                if *inferred {
                    format!("literal `{}{}` is out of range for `{}`", sign, raw, suffix)
                } else {
                    format!("literal `{}{}{}` is out of range for `{}`", sign, raw, suffix, suffix)
                }
            }

            TypeError::CannotInferType { .. } =>
                "cannot infer type — add an explicit type annotation".to_string(),

            TypeError::GenericArgCountMismatch { type_name, expected, found, .. } =>
                format!(
                    "`{}` expects {} type argument(s), found {}",
                    type_name, expected, found
                ),

            TypeError::UnknownVariant { enum_name, variant_name, .. } =>
                format!("enum `{}` has no variant `{}`", enum_name, variant_name),

            TypeError::VariantArityMismatch { enum_name, variant_name, expected, found, .. } =>
                format!(
                    "`{}.{}` expects {} value(s), found {}",
                    enum_name, variant_name, expected, found
                ),

            TypeError::NonExhaustiveMatch { missing_variants, .. } =>
                format!(
                    "match is not exhaustive — missing variant(s): {}",
                    missing_variants.join(", ")
                ),

            TypeError::MixedDiscriminantAndPayload { enum_name, .. } =>
                format!(
                    "enum `{}` mixes an explicit discriminant variant with a payload-carrying variant",
                    enum_name
                ),

            TypeError::InlineListCapacityNotLiteral { .. } =>
                "InlineList.new(capacity) requires a literal integer capacity".to_string(),
        }
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            TypeError::CannotInferType { suggestion: Some(s), .. } =>
                Some(s.clone()),

            TypeError::NonExhaustiveMatch { .. } =>
                Some("add arms for the missing variant(s), or a wildcard `_ => ...` arm to cover the rest".to_string()),

            TypeError::MixedDiscriminantAndPayload { .. } =>
                Some("use either explicit discriminants on every fieldless variant, or payload variants — not both in the same enum".to_string()),

            TypeError::InlineListCapacityNotLiteral { .. } =>
                Some("write the capacity as a plain integer literal, e.g. InlineList.new(64) — a variable or computed expression can't be used here".to_string()),

            TypeError::InvalidFormatSpec { spec_part, .. } if spec_part == "precision" =>
                Some("`.precision` only applies to float/double (decimal places) or string (max length) — drop it, or format a value of one of those types".to_string()),

            TypeError::InvalidFormatSpec { spec_part, .. } if spec_part == "sign (+)" =>
                Some("`+` only applies to int/float/double, drop it or format a numeric value".to_string()),

            TypeError::InvalidFormatSpec { spec_part, .. } if spec_part == "zero-pad (0)" =>
                Some("zero-padding only applies to int/float/double, drop the leading 0 or format a numeric value".to_string()),

            TypeError::InvalidFormatSpec { spec_part, .. } if spec_part == "numeric base" =>
                Some("`x`/`X`/`o`/`b` only apply to int, drop it or format an int value".to_string()),

            TypeError::AlternateFormatWithoutBase { .. } =>
                Some("add a trailing base, e.g. `{value:#x}`, or drop the `#`".to_string()),

            TypeError::UnknownDeriveTrait { trait_name, .. }
                if trait_name == "Debug" || trait_name == "Display" || trait_name == "PartialEq" =>
                Some(format!("drop `@derive({})` — it has no effect", trait_name)),

            TypeError::UnknownDeriveTrait { .. } =>
                Some("supported derive traits: `PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`".to_string()),

            TypeError::DeriveRequiresOther { trait_name, requires, .. } =>
                Some(format!("add `@derive({}, {})`", requires, trait_name)),

            TypeError::TypeNotOrderable { .. } =>
                Some("add `@derive(PartialOrd)` (or `@derive(Ord)`) to the struct, or compare a different field".to_string()),

            TypeError::IntLiteralOutOfRange { suffix, inferred, .. } =>
                Some(if *inferred {
                    format!("use a value that fits `{}`, or give the binding a wider type", suffix)
                } else {
                    format!("use a smaller value, drop the `{}` suffix, or pick a wider type", suffix)
                }),

            _ => None,
        }
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for TypeError {}

impl crate::error_management::render::Diagnosable for TypeError {
    // See docs/DIAGNOSTICS_RULES.md, "Error Code Registry" — TYPE-1xx.
    // TYPE-2xx (tier & arena enforcement) is now `TierError`, its own
    // enum in `../tier/mod.rs`, with its own TIER-0xx range.
    fn code(&self) -> &'static str {
        match self {
            TypeError::TypeMismatch { .. }              => "TYPE-101",
            TypeError::ArgumentCountMismatch { .. }     => "TYPE-102",
            TypeError::NoSuchField { .. }                => "TYPE-103",
            TypeError::NoSuchMethod { .. }               => "TYPE-104",
            TypeError::TryOnNonFallible { .. }           => "TYPE-105",
            TypeError::AwaitOnNonTask { .. }             => "TYPE-106",
            TypeError::CannotInferType { .. }            => "TYPE-107",
            TypeError::GenericArgCountMismatch { .. }    => "TYPE-108",
            TypeError::UnknownVariant { .. }             => "TYPE-109",
            TypeError::VariantArityMismatch { .. }       => "TYPE-110",
            TypeError::NonExhaustiveMatch { .. }         => "TYPE-111",
            TypeError::MixedDiscriminantAndPayload { .. } => "TYPE-112",
            TypeError::InlineListCapacityNotLiteral { .. } => "TYPE-113",
            TypeError::DerefOnNonReference { .. }          => "TYPE-114",
            TypeError::InvalidFormatSpec { .. }            => "TYPE-115",
            TypeError::AlternateFormatWithoutBase { .. }   => "TYPE-119",
            TypeError::UnknownDeriveTrait { .. }            => "TYPE-116",
            TypeError::DeriveRequiresOther { .. }           => "TYPE-117",
            TypeError::TypeNotOrderable { .. }              => "TYPE-118",
            TypeError::IntLiteralOutOfRange { .. }          => "TYPE-120",
        }
    }
    fn span(&self) -> Span { self.span() }
    fn message(&self) -> String { self.message() }
    fn suggestion(&self) -> Option<String> { self.suggestion() }

    fn secondary_spans(&self) -> Vec<(Span, String)> {
        match self {
            TypeError::TypeMismatch { because_of: Some(span), .. } =>
                vec![(*span, "expected type was established here".to_string())],
            _ => Vec::new(),
        }
    }
}
