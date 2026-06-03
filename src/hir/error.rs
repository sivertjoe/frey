use std::fmt;

use crate::{hir::types::Ty, lexer::types::Span};

#[derive(Debug, PartialEq)]
pub struct Error {
    pub span: Span,
    pub kind: ErrorKind,
}

#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    NameNotFound {
        name: String,
    },
    AlreadyDefined {
        name: String,
    },
    NotCallable {
        found: Ty,
    },
    NotIndexable {
        found: Ty,
    },
    EmptyArrayLiteral,
    LiteralOutOfRange {
        value: i32,
        target: Ty,
    },
    NotDereferencable {
        found: Ty,
    },
    UnknownType {
        name: String,
    },
    UnknownField {
        struct_name: String,
        field: String,
    },
    MissingFields {
        struct_name: String,
        missing: Vec<String>,
    },
    DuplicateField {
        struct_name: String,
        field: String,
    },
    GenericIsAlsoAStruct {
        name: String,
    },
    GenericAlreadyDefined {
        name: String,
    },
    GenericOutsideFunctionSignature {
        name: String,
    },
    CannotInferTypeArg {
        name: String,
    },
    TypeMismatch {
        expected: Ty,
        found: Ty,
    },
    NotAStruct {
        found: Ty,
    },
    DirectStructRecursion {
        name: String,
    },
    StructDefNotAllowedHere,
    MissingTypeArguments {
        name: String,
    },
    UnexpectedTypeArguments {
        name: String,
    },
    TypeArgArityMismatch {
        name: String,
        expected: usize,
        found: usize,
    },
    /// Raised by a `comperror(...)` call reached during comptime evaluation.
    ComptimeError {
        message: String,
    },
    /// No overload of `name` matches the argument types at a call site.
    NoMatchingOverload {
        name: String,
    },
    /// Multiple overloads of `name` match the argument types.
    AmbiguousOverload {
        name: String,
    },
    /// Tuple field access (`t.0`, `t.1`, ...) was attempted on a non-tuple
    /// value.
    NotATuple {
        found: Ty,
    },
    /// A tuple index `t.N` is out of range for the tuple's length.
    TupleIndexOutOfRange {
        len: usize,
        index: usize,
    },
    /// `let f: T = (...) -> ... {};` — function literals carry their own sig.
    TypeAnnotationNotAllowed,
    /// Two enums declared a variant with the same name (variants are global).
    DuplicateVariant {
        name: String,
        other_enum: String,
    },
    /// A pattern's variant name doesn't belong to the scrutinee's enum.
    UnknownVariant {
        name: String,
    },
    /// A variant pattern bound the wrong number of fields.
    VariantArityMismatch {
        name: String,
        expected: usize,
        found: usize,
    },
    /// A `match` expression doesn't cover every variant and has no wildcard.
    NonExhaustiveMatch {
        missing: Vec<String>,
    },
    /// A `match` arm's variant repeats one already covered earlier.
    DuplicateMatchArm {
        name: String,
    },
    /// Match scrutinee isn't an enum.
    MatchOnNonEnum {
        found: Ty,
    },
    /// Nullary variant of a generic enum with no way to infer its type args.
    CannotInferEnumTypeArg {
        variant: String,
    },
    /// A `{x : body}` closure had no expected function type to drive
    /// parameter inference.
    ClosureTypeUnknown,
    /// Closure parameter count doesn't match the expected function arity.
    ClosureArityMismatch {
        expected: usize,
        found: usize,
    },
    /// `let x;` — no value and no `: T`, nothing to zero-initialize against.
    MissingTypeForZeroInit,
    /// `extern (...)` was used outside a top-level `let name = extern (...);`.
    ExternMustBeTopLevel,
    /// `null` was used in a position where the expected pointer type isn't
    /// known. Add an annotation (`let x: *FILE = null;`) or compare against
    /// a typed value (`if fp == null { ... }` works when `fp` is `*FILE`).
    CannotInferNullType,
    /// `name<T, U>` used in value position where `name` doesn't resolve to a
    /// generic function template.
    NotAGenericFunction {
        name: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.kind, self.span.start.line, self.span.start.column
        )
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::NameNotFound { name } => {
                write!(f, "name not found: {name}")
            }
            ErrorKind::AlreadyDefined { name } => {
                write!(f, "name `{name}` already defined")
            }
            ErrorKind::NotCallable { found } => {
                write!(f, "cannot call non-function value of type {found:?}")
            }
            ErrorKind::NotIndexable { found } => {
                write!(f, "cannot subscript non-array value of type {found:?}")
            }
            ErrorKind::EmptyArrayLiteral => {
                write!(
                    f,
                    "empty array literals are not supported (element type cannot be inferred)"
                )
            }
            ErrorKind::LiteralOutOfRange { value, target } => {
                write!(f, "literal {value} is out of range for type {target:?}")
            }
            ErrorKind::NotDereferencable { found } => {
                write!(f, "cannot dereference non-pointer value of type {found:?}")
            }
            ErrorKind::UnknownType { name } => {
                write!(f, "unknown type `{name}`")
            }
            ErrorKind::UnknownField { struct_name, field } => {
                write!(f, "struct `{struct_name}` has no field `{field}`")
            }
            ErrorKind::MissingFields {
                struct_name,
                missing,
            } => {
                write!(
                    f,
                    "struct literal for `{struct_name}` is missing fields: {}",
                    missing.join(", ")
                )
            }
            ErrorKind::DuplicateField { struct_name, field } => {
                write!(
                    f,
                    "field `{field}` is specified more than once in `{struct_name}` literal"
                )
            }
            ErrorKind::NotAStruct { found } => {
                write!(f, "field access requires a struct, got {found:?}")
            }
            ErrorKind::DirectStructRecursion { name } => {
                write!(
                    f,
                    "struct `{name}` directly contains itself (infinite size); use `*{name}` for self-references"
                )
            }
            ErrorKind::StructDefNotAllowedHere => {
                write!(
                    f,
                    "struct definitions are only allowed as `let X = struct {{...}};`"
                )
            }
            ErrorKind::GenericIsAlsoAStruct { name } => {
                write!(f, "the generic type {name} is already defined as a struct")
            }
            ErrorKind::GenericAlreadyDefined { name } => {
                write!(
                    f,
                    "generic type `${name}` is already declared in this function; use `{name}` to refer to it"
                )
            }
            ErrorKind::GenericOutsideFunctionSignature { name } => {
                write!(
                    f,
                    "generic type `${name}` can only be declared in a function signature"
                )
            }
            ErrorKind::CannotInferTypeArg { name } => {
                write!(f, "cannot infer type for generic argument `{name}`")
            }
            ErrorKind::TypeMismatch { expected, found } => {
                write!(f, "type mismatch: expected {expected:?}, found {found:?}")
            }
            ErrorKind::MissingTypeArguments { name } => {
                write!(
                    f,
                    "struct `{name}` is generic; provide type arguments like `{name}<...>`"
                )
            }
            ErrorKind::UnexpectedTypeArguments { name } => {
                write!(
                    f,
                    "struct `{name}` is not generic and takes no type arguments"
                )
            }
            ErrorKind::TypeArgArityMismatch {
                name,
                expected,
                found,
            } => {
                write!(
                    f,
                    "struct `{name}` expects {expected} type argument(s), got {found}"
                )
            }
            ErrorKind::ComptimeError { message } => {
                write!(f, "{message}")
            }
            ErrorKind::NoMatchingOverload { name } => {
                write!(f, "no overload of `{name}` matches these arguments")
            }
            ErrorKind::AmbiguousOverload { name } => {
                write!(f, "ambiguous call to `{name}`: multiple overloads match")
            }
            ErrorKind::NotATuple { found } => {
                write!(f, "tuple index access requires a tuple, got {found:?}")
            }
            ErrorKind::TupleIndexOutOfRange { len, index } => {
                write!(
                    f,
                    "tuple index {index} out of range for tuple of length {len}"
                )
            }
            ErrorKind::TypeAnnotationNotAllowed => {
                write!(
                    f,
                    "type annotations are not allowed on function declarations; the signature comes from the function itself"
                )
            }
            ErrorKind::DuplicateVariant { name, other_enum } => {
                write!(
                    f,
                    "variant `{name}` is already declared in `enum {other_enum}`; variant names must be unique across all enums"
                )
            }
            ErrorKind::UnknownVariant { name } => {
                write!(f, "no enum variant named `{name}`")
            }
            ErrorKind::VariantArityMismatch {
                name,
                expected,
                found,
            } => {
                write!(
                    f,
                    "variant `{name}` takes {expected} field(s), but the pattern binds {found}"
                )
            }
            ErrorKind::NonExhaustiveMatch { missing } => {
                write!(
                    f,
                    "non-exhaustive match: missing variant(s) {}; add an arm or use `_` to catch the rest",
                    missing.join(", ")
                )
            }
            ErrorKind::DuplicateMatchArm { name } => {
                write!(f, "variant `{name}` is covered by more than one arm")
            }
            ErrorKind::MatchOnNonEnum { found } => {
                write!(f, "`match` requires an enum value, got {found:?}")
            }
            ErrorKind::CannotInferEnumTypeArg { variant } => {
                write!(
                    f,
                    "cannot infer type arguments for `{variant}`; provide them explicitly like `{variant}<Type>`"
                )
            }
            ErrorKind::ClosureTypeUnknown => {
                write!(
                    f,
                    "cannot infer this closure's parameter types; use it where an expected function type is known"
                )
            }
            ErrorKind::ClosureArityMismatch { expected, found } => {
                write!(
                    f,
                    "closure has {found} parameter(s), but {expected} were expected"
                )
            }
            ErrorKind::MissingTypeForZeroInit => {
                write!(
                    f,
                    "`let name;` requires a `: T` annotation so the zero-initializer has a layout"
                )
            }
            ErrorKind::ExternMustBeTopLevel => {
                write!(
                    f,
                    "`extern (...)` is only allowed at top level, in `let name = extern (...);`"
                )
            }
            ErrorKind::CannotInferNullType => {
                write!(
                    f,
                    "cannot infer the pointer type of `null` from context; add a `: *T` annotation"
                )
            }
            ErrorKind::NotAGenericFunction { name } => {
                write!(
                    f,
                    "`{name}<...>` requires `{name}` to be a generic function"
                )
            }
        }
    }
}

impl std::error::Error for ErrorKind {}
