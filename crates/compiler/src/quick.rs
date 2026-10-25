//! Thin CLI/test wrapper over `coqc_format::parse` (the real Scryfall-syntax parser); drops sort/unique directives.
use coqc_format::search::Expr;

pub fn parse(q: &str) -> Result<Option<Expr>, String> {
    coqc_format::parse::parse(q).map(|q| q.expr).map_err(|e| e.0)
}
