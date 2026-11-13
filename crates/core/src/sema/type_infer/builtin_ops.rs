// src/sema/type_infer/builtin_ops.rs
//! Operators, the call of an integer literal and tuple indexes on built-in
//! types, checked at compile time.
//!
//! The run time supports a fixed set of operand types for each operator
//! (`eval_binop`, `bitwise_op`, the unary arms). Everything else used to pass
//! sema and panic when it ran: `[1] + [2]`, `true & false`, `-x` for a `u8`,
//! `1.5 & 2.5`, `!5`, `n()` for an `int`. A user type is checked through its
//! operator trait (`operator_site`, TYPE-136); the types here have no such
//! trait, so a plain table of what each operator accepts is the whole rule.
//!
//! | operator | accepted operand types |
//! |---|---|
//! | `+` `+=` | every number, and a string with a string |
//! | `-` `*` `/` `%` and their `=` forms | every number |
//! | `&` `\|` `^` `<<` `>>` and their `=` forms, unary `~` | every integer type |
//! | unary `-` | every signed integer and every float |
//!
//! A type that is not settled yet (a variable, `unknown`, a type parameter, a
//! `dyn`, a struct or an enum) is never reported here: "not known yet" is not
//! "known to be wrong".

use super::*;

/// How an operand type is classified for the table above.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// `int`, `long`, `i8` to `i64`, `isize`, and an unsuffixed literal.
    Signed,
    /// `uint`, `ulong`, `u8` to `u64`, `usize`.
    Unsigned,
    /// `float`, `double`, `f32`, `f64`.
    Float,
    Str,
    /// A built-in type no arithmetic or bitwise operator accepts.
    Other,
}

impl<'a> InferCtx<'a> {
    /// The class of `ty`, or `None` when the type is not settled or is one the
    /// operator traits handle (a user type, a type parameter, a `dyn`).
    fn operand_class(&mut self, ty: TypeId) -> Option<Class> {
        let t = self.peel_wrappers(ty);
        let t = self.apply(t);
        if t.is_error() { return None; }
        Some(match self.ctx.types.get(t) {
            SemaType::Int | SemaType::Long
            | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
            | SemaType::Isize                                           => Class::Signed,
            SemaType::Uint | SemaType::Ulong
            | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
            | SemaType::Usize                                           => Class::Unsigned,
            SemaType::Float | SemaType::Double | SemaType::F32 | SemaType::F64 => Class::Float,
            SemaType::Str                                               => Class::Str,
            SemaType::Var(n) if self.int_lit_vars.contains_key(n)       => Class::Signed,
            SemaType::Bool | SemaType::Char | SemaType::Void
            | SemaType::List(_) | SemaType::Dictionary(_, _) | SemaType::Set(_)
            | SemaType::Queue(_) | SemaType::Stack(_) | SemaType::InlineList(_)
            | SemaType::Linqerizer(_) | SemaType::Tuple(_)
            | SemaType::Array { .. } | SemaType::Slice(_)
            | SemaType::Pool(_) | SemaType::Handle(_)
            | SemaType::Fallible(_) | SemaType::Task(_) | SemaType::Optional(_) => Class::Other,
            _ => return None,
        })
    }

    /// Whether the operator named by `trait_name` (`"Add"`, `"Neg"`, ...) is
    /// defined for `ty`. Reports `TYPE-139` and returns `false` when it is not.
    /// `op` is the operator as written (`+`, `<<=`, `~`).
    pub(super) fn check_builtin_operator(
        &mut self, trait_name: &str, op: &'static str, ty: TypeId, span: Span,
    ) -> bool {
        let Some(class) = self.operand_class(ty) else { return true; };
        let accepted = match trait_name {
            "Add"                                    => class != Class::Other,
            "Sub" | "Mul" | "Div" | "Rem"            => matches!(class, Class::Signed | Class::Unsigned | Class::Float),
            "BitAnd" | "BitOr" | "BitXor"
            | "Shl" | "Shr" | "BitNot"               => matches!(class, Class::Signed | Class::Unsigned),
            "Neg"                                    => matches!(class, Class::Signed | Class::Float),
            _                                        => true,
        };
        if accepted { return true; }
        let resolved = self.apply(ty);
        let on_type = self.display_type(resolved);
        self.errors.add_type_error(TypeError::OperatorNotSupported { op, on_type, span });
        false
    }

    /// Whether a value of type `callee_ty` can be called, for the one case the
    /// general check in `call_return_type` never sees: an unsuffixed integer
    /// literal (`let n = 5` then `n()`, or `5()`). Its type is a variable until
    /// something fixes it, and a variable is waited on, not reported, so the
    /// call would pass sema and panic when it ran. Every other settled type
    /// that is not a function is reported there (`TYPE-140`). Reports and
    /// returns `false` for the literal.
    pub(super) fn check_callable(&mut self, callee_ty: TypeId, span: Span) -> bool {
        let t = self.peel_wrappers(callee_ty);
        let t = self.apply(t);
        let is_int_literal = matches!(self.ctx.types.get(t), SemaType::Var(n) if self.int_lit_vars.contains_key(n));
        if !is_int_literal { return true; }
        let on_type = self.display_type(t);
        self.errors.add_type_error(TypeError::NotCallable { on_type, span });
        false
    }

    /// A literal index past the end of a tuple of known size (`TYPE-141`). The
    /// run time panics on it; the size is part of the type, so it is known here.
    pub(super) fn report_tuple_index(&mut self, tuple: TypeId, len: usize, index: String, span: Span) {
        let tuple = self.display_type(tuple);
        self.errors.add_type_error(TypeError::TupleIndexOutOfRange { index, len, tuple, span });
    }

    /// A struct literal written with the name of an alias whose target is not
    /// a struct or an enum (`type Score = int` then `Score { x = 1 }`,
    /// `TYPE-142`). `alias` is the alias definition.
    pub(super) fn report_alias_not_a_struct(&mut self, name: &str, alias: DefId, span: Span) {
        let target = self.alias_expansions.get(&alias).map(|e| e.target);
        let target = match target {
            Some(t) => self.display_type(t),
            None    => "its target".to_string(),
        };
        self.errors.add_type_error(TypeError::NotAStruct { name: name.to_string(), target, span });
    }
}
