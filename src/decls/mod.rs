//! Declarations of code outside the repo: the Go standard library and modules
//! (`go`), and TypeScript `.d.ts` files (`dts`). Read on demand, held for one run.

/// A type named by a declaration, qualified by the package it comes from.
///
/// - A named type has `pkg` set to its import path (Go) or bare specifier (TS),
///   and `None` for builtins (`error`, `string`), type parameters and DOM globals.
///   Type arguments go in `elems`.
/// - `*T` sets `pointer` on T. Deeper pointers fall back to source text.
/// - Composites name their shape and put their parts in `elems`: `"[]"` (Go
///   slice or array, TS `T[]`), `"map"` [K, V], `"chan"` [T], `"..."` [T].
/// - Anything else (Go `func(...)`, TS unions and object literals) is `name` =
///   the source text, `pkg` None, `elems` empty.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeRef {
    pub pkg: Option<String>,
    pub name: String,
    pub pointer: bool,
    pub elems: Vec<TypeRef>,
}
