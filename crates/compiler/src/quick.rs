//! Tiny Scryfall-ish query parser for the CLI and tests ONLY (the app has its own searchParser, which
//! should emit `coqc_format::search::Expr` JSON). Supports: bare words/"phrases" = name, `-` negation,
//! `or` between terms (AND binds tighter), keys t o c id mv r s cn name, ops `: = != < <= > >=`.
use coqc_format::search::*;

fn tokens(q: &str) -> Vec<String> {
    let (mut out, mut cur, mut inq) = (Vec::new(), String::new(), false);
    for c in q.chars() {
        match c {
            '"' => inq = !inq,
            c if c.is_whitespace() && !inq => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn split_op(t: &str) -> Option<(&str, Op, &str)> {
    let i = t.find(|c: char| matches!(c, ':' | '=' | '!' | '<' | '>'))?;
    let rest = &t[i..];
    for (s, op) in [(">=", Op::Ge), ("<=", Op::Le), ("!=", Op::Ne), (":", Op::Colon), ("=", Op::Eq), ("<", Op::Lt), (">", Op::Gt)] {
        if rest.starts_with(s) {
            return Some((&t[..i], op, &rest[s.len()..]));
        }
    }
    None
}

fn term(t: &str) -> Result<Expr, String> {
    let Some((k, op, v)) = split_op(t) else { return Ok(Expr::Name(t.to_string())) };
    let v = v.to_string();
    Ok(match k.to_ascii_lowercase().as_str() {
        "t" | "type" => Expr::Type(v),
        "o" | "oracle" => Expr::Oracle(v),
        "n" | "name" => Expr::Name(v),
        "c" | "color" => Expr::Color(ColorCmp { op, colors: v }),
        "id" | "identity" => Expr::Identity(ColorCmp { op, colors: v }),
        "mv" | "cmc" => Expr::Mv(MvCmp { op, value: v.parse().map_err(|_| format!("bad number: {v}"))? }),
        "r" | "rarity" => Expr::Rarity(RarityCmp { op, value: v }),
        "s" | "set" | "e" => Expr::Set(v),
        "cn" => Expr::Cn(v),
        _ => Expr::Name(t.to_string()),
    })
}

pub fn parse(q: &str) -> Result<Option<Expr>, String> {
    let (mut groups, mut cur): (Vec<Expr>, Vec<Expr>) = (Vec::new(), Vec::new());
    let flush = |cur: &mut Vec<Expr>, groups: &mut Vec<Expr>| {
        if !cur.is_empty() {
            groups.push(Expr::And(std::mem::take(cur)));
        }
    };
    for t in tokens(q) {
        if t.eq_ignore_ascii_case("or") {
            flush(&mut cur, &mut groups);
        } else if let Some(r) = t.strip_prefix('-').filter(|r| !r.is_empty()) {
            cur.push(Expr::Not(Box::new(term(r)?)));
        } else {
            cur.push(term(&t)?);
        }
    }
    flush(&mut cur, &mut groups);
    Ok(match groups.len() {
        0 => None,
        1 => groups.pop(),
        _ => Some(Expr::Or(groups)),
    })
}
