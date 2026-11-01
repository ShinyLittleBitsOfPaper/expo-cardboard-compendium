//! Thin CLI/test wrapper over `crate::parse` (the real Scryfall-syntax parser); drops sort/unique directives.
use crate::query::Expr;

pub fn parse(q: &str) -> Result<Option<Expr>, String> {
    crate::parse::parse(q).map(|q| q.expr).map_err(|e| e.0)
}
