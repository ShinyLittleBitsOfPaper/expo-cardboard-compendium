//! MTG side of the generic index: derived columns (computed from the `Store`), `Custom` predicates, and the translation of the
//! Scryfall query tree (`crate::query::Query`) into the generic one (`coqc_format::gindex::GQuery`).
use coqc_format::gindex::*;
use crate::query::*;
use coqc_format::table::{ColData, Store};
use crate::types::OracleRecord;
use coqc_format::search::{fold, num_cmp, Bits, Op};
use crate::tables;
use crate::types::decode_nibbles;
use coqc_format::codec::Reader;
use std::collections::HashMap;

pub const TEXT_GROUP: &str = "text";

/// Column builder for [`derive_store`].
#[derive(Default)]
struct Acc {
    name_plain: Vec<String>,
    name_n: Vec<String>,
    name_sort: Vec<String>,
    type_f: Vec<String>,
    text: Vec<String>,
    full: Vec<String>,
    colors: Vec<u32>,
    identity: Vec<u32>,
    set_f: Vec<String>,
    cn_f: Vec<String>,
    hidden: Vec<bool>,
    rank: Vec<Option<f64>>,
    rar_s: Vec<String>,
    cn_num: Vec<Option<f64>>,
    mana_f: Vec<String>,
    pt_f: Vec<String>,
    is_o: Vec<u32>,
    is_p: Vec<u32>,
}

fn bit_o(name: &str) -> u32 {
    IS_O.iter().position(|n| *n == name).map_or(0, |i| 1 << i)
}
fn bit_p(name: &str) -> u32 {
    IS_P.iter().position(|n| *n == name).map_or(0, |i| 1 << i)
}

/// `power/toughness/loyalty` of one face or card (raw strings, empty when absent).
fn pt_triplet(extra: Option<&str>) -> String {
    let v = extra.and_then(|x| serde_json::from_str::<serde_json::Value>(x).ok());
    let g = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|x| x.as_str().map(String::from).or_else(|| x.as_f64().map(|n| n.to_string())))
            .unwrap_or_default()
    };
    format!("{}/{}/{}", g("power"), g("toughness"), g("loyalty"))
}

/// Per-face stats joined by ` // ` (a card without faces has one entry).
fn pt_of(o: &OracleRecord) -> String {
    let top = pt_triplet(o.extra.as_deref());
    match &o.faces {
        Some(fs) if !fs.is_empty() => {
            let v: Vec<String> = fs.iter().map(|f| pt_triplet(f.extra.as_deref())).collect();
            if v.iter().all(|x| x == "//") && top != "//" { top } else { v.join(" // ") }
        }
        _ => top,
    }
}

/// Lowercase mana cost(s), faces joined by ` // `.
fn mana_of(o: &OracleRecord) -> String {
    match &o.faces {
        Some(fs) if !fs.is_empty() => fs.iter().map(|f| f.mana_cost.clone().unwrap_or_default()).collect::<Vec<_>>().join(" // "),
        _ => o.mana_cost.clone().unwrap_or_default(),
    }
    .to_ascii_lowercase()
}

/// `{2}{r}{r}` -> sorted symbols.
fn symbols(s: &str) -> Vec<String> {
    let (mut v, mut rest) = (Vec::new(), s);
    while let Some(a) = rest.find('{') {
        let Some(b) = rest[a..].find('}') else { break };
        v.push(rest[a..a + b + 1].to_string());
        rest = &rest[a + b + 1..];
    }
    v.sort();
    v
}

fn counts(v: &[String]) -> HashMap<&str, usize> {
    let mut m = HashMap::new();
    for x in v {
        *m.entry(x.as_str()).or_insert(0) += 1;
    }
    m
}

/// Multiset comparison of a card's cost with the query's (`:`/`>=` card contains query, `<=` the reverse, `=` equal).
fn mana_cmp(card: &[String], q: &[String], op: Op) -> bool {
    let (cc, qc) = (counts(card), counts(q));
    let sup = qc.iter().all(|(k, n)| cc.get(k).copied().unwrap_or(0) >= *n);
    let sub = cc.iter().all(|(k, n)| qc.get(k).copied().unwrap_or(0) >= *n);
    let eq = sup && sub;
    match op {
        Op::Colon | Op::Ge => sup,
        Op::Gt => sup && !eq,
        Op::Eq => eq,
        Op::Le => sub,
        Op::Lt => sub && !eq,
        Op::Ne => !eq,
    }
}

/// Numeric value of a power/toughness/loyalty string (`*` = 0, `1+*` = 1, as Scryfall).
fn stat_num(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }
    let d: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    d.parse().ok().or_else(|| s.contains('*').then_some(0.0))
}

fn stat_idx(s: &str) -> Option<usize> {
    match s {
        "pow" => Some(0),
        "tou" => Some(1),
        "loy" => Some(2),
        _ => None,
    }
}

pub fn op_sym(op: Op) -> &'static str {
    match op {
        Op::Colon => ":",
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    }
}

fn parse_op(s: &str) -> Result<Op, String> {
    [Op::Colon, Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge].into_iter().find(|o| op_sym(*o) == s).ok_or_else(|| format!("bad operator {s}"))
}

fn has_keyword(extra: &str, kw: &str) -> bool {
    let Some(a) = extra.find("\"keywords\":[") else { return false };
    let rest = &extra[a + 12..];
    let Some(b) = rest.find(']') else { return false };
    rest[..b].split(',').any(|k| k.trim().trim_matches('"').eq_ignore_ascii_case(kw))
}

impl Acc {
    fn oracle(&mut self, o: &OracleRecord) {
        let name = o.name.clone().unwrap_or_default();
        self.name_plain.push(fold(&name));
        self.name_n.push(fold_name(&name));
        self.name_sort.push(fold(&name).chars().filter(|c| c.is_alphanumeric()).collect::<String>());
        let type_f = fold(&join_faces(&o.type_line, &o.faces, |f| &f.type_line));
        let f = join_faces(&o.oracle_text, &o.faces, |f| &f.oracle_text);
        let full = fold(&f);
        let colors = oracle_mask(o, o.colors, "colors", true) as u32;
        self.colors.push(colors);
        self.identity.push(oracle_mask(o, o.color_identity, "color_identity", false) as u32);
        let mana = mana_of(o);
        let has = |w: &str| type_f.contains(w);
        let mut b = 0u32;
        if ["artifact", "creature", "enchantment", "land", "planeswalker", "battle"].iter().any(|w| has(w)) {
            b |= bit_o("permanent");
        }
        if !has("land") {
            b |= bit_o("spell");
        }
        if has("legendary") || has("artifact") || has("saga") {
            b |= bit_o("historic");
        }
        if has("creature") && full.trim().is_empty() {
            b |= bit_o("vanilla");
        }
        if (has("legendary") && has("creature")) || full.contains("can be your commander") {
            b |= bit_o("commander");
        }
        if colors.count_ones() >= 2 {
            b |= bit_o("gold");
        }
        let syms = symbols(&mana);
        if syms.iter().any(|x| x.contains('/') && !x.ends_with("/p}")) {
            b |= bit_o("hybrid");
        }
        if syms.iter().any(|x| x.ends_with("/p}")) {
            b |= bit_o("phyrexian");
        }
        if o.extra.as_deref().is_some_and(|x| x.contains("\"reserved\":true")) {
            b |= bit_o("reserved");
        }
        self.is_o.push(b);
        self.mana_f.push(mana);
        self.pt_f.push(pt_of(o));
        self.type_f.push(type_f);
        self.text.push(fold(&strip_reminder(&f)));
        self.full.push(full);
    }

    #[allow(clippy::too_many_arguments)]
    fn printing(&mut self, layout: Option<&str>, extra: Option<&str>, playtest: bool, set: Option<&str>, cn: Option<&str>, rarity: Option<&str>, flags: (Option<i64>, Option<bool>, Option<bool>, Option<bool>)) {
        let set_type = extra.and_then(json_str_field_set_type);
        let mut p = 0u32;
        let fin = flags.0.unwrap_or(0);
        for (m, n) in [(2, "foil"), (1, "nonfoil"), (4, "etched")] {
            if fin & m != 0 {
                p |= bit_p(n);
            }
        }
        for (v, n) in [(flags.1, "promo"), (flags.2, "reprint"), (flags.3, "digital")] {
            if v == Some(true) {
                p |= bit_p(n);
            }
        }
        for (k, n) in [("full_art", "fullart"), ("textless", "textless"), ("oversized", "oversized"), ("variation", "variation"), ("story_spotlight", "spotlight")] {
            if extra.is_some_and(|x| x.contains(&format!("\"{k}\":true"))) {
                p |= bit_p(n);
            }
        }
        if set_type == Some("funny") {
            p |= bit_p("funny");
        }
        match layout {
            Some(l @ ("split" | "flip" | "transform" | "meld" | "leveler" | "saga" | "adventure" | "planar" | "scheme" | "vanguard" | "augment" | "host")) => p |= bit_p(l),
            Some("modal_dfc") => p |= bit_p("mdfc"),
            Some("token" | "double_faced_token" | "emblem") => p |= bit_p("token"),
            _ => {}
        }
        if matches!(layout, Some("transform" | "modal_dfc" | "meld" | "double_faced_token" | "reversible_card")) {
            p |= bit_p("dfc");
        }
        self.is_p.push(p);
        self.hidden.push(matches!(layout, Some("vanguard" | "planar" | "scheme" | "phenomenon" | "art_series")) || matches!(set_type, Some("memorabilia")) || playtest);
        self.set_f.push(set.map(fold).unwrap_or_default());
        let cn = cn.map(fold).unwrap_or_default();
        let d: String = cn.chars().take_while(|c| c.is_ascii_digit()).collect();
        self.cn_num.push(d.parse::<f64>().ok());
        self.cn_f.push(cn);
        self.rar_s.push(rarity.map(String::from).unwrap_or_default());
        self.rank.push(rarity.and_then(rarity_rank).map(|r| r as f64));
    }

    fn finish(self) -> Vec<DerivedCol> {
        let t = |table: &str, name: &str, v: Vec<String>, g: Option<&str>| DerivedCol { table: table.into(), name: name.into(), data: DData::Text(v), tri_group: g.map(String::from) };
        vec![
            t("oracle", "name_plain", self.name_plain, None),
            t("oracle", "name_n", self.name_n, Some("name")),
            t("oracle", "name_sort", self.name_sort, None),
            t("oracle", "type_f", self.type_f, None),
            t("oracle", "text", self.text, Some(TEXT_GROUP)),
            t("oracle", "full_text", self.full, Some(TEXT_GROUP)),
            DerivedCol { table: "oracle".into(), name: "colors_m".into(), data: DData::Mask(self.colors), tri_group: None },
            DerivedCol { table: "oracle".into(), name: "identity_m".into(), data: DData::Mask(self.identity), tri_group: None },
            t("oracle", "mana_f", self.mana_f, None),
            t("oracle", "pt_f", self.pt_f, None),
            DerivedCol { table: "oracle".into(), name: "is_o".into(), data: DData::Mask(self.is_o), tri_group: None },
            DerivedCol { table: "printing".into(), name: "is_p".into(), data: DData::Mask(self.is_p), tri_group: None },
            t("printing", "set_f", self.set_f, None),
            t("printing", "cn_f", self.cn_f, None),
            t("printing", "rarity_s", self.rar_s, None),
            DerivedCol { table: "printing".into(), name: "hidden".into(), data: DData::Flag(self.hidden), tri_group: None },
            DerivedCol { table: "printing".into(), name: "rarity_rank".into(), data: DData::Num(self.rank), tri_group: None },
            DerivedCol { table: "printing".into(), name: "cn_num".into(), data: DData::Num(self.cn_num), tri_group: None },
        ]
    }
}

/// Schema `Text` columns the MTG dialect never searches by their own name (it searches the derived, folded variants instead).
pub const SKIP_AUTO: [&str; 6] = ["oracle.name", "oracle.type_line", "oracle.oracle_text", "printing.set_name", "printing.flavor_text", "printing.artist"];

/// The derived index columns, computed straight from a store with the MTG schema (no `Dataset` is built).
pub fn derive_store(st: &Store) -> Result<Vec<DerivedCol>, String> {
    let e = |e: coqc_format::Error| e.to_string();
    tables::check_schema(st).map_err(e)?;
    let mut a = Acc::default();
    for i in 0..st.tables[0].rows() {
        a.oracle(&tables::oracle_at(st, i).map_err(e)?);
    }
    let meta = tables::meta_of(st);
    let rarities: Vec<String> = meta.rarities.values().iter().map(|r| r.to_string()).collect();
    let layouts = meta.layouts.values();
    let promo_types = meta.promo_types.values();
    let bad = || "column type".to_string();
    let col = |c: usize| &st.tables[2].cols[c];
    let (ColData::Enum(layout_c), ColData::Json(extra_c), ColData::Set(ptypes_c), ColData::Str(set_c), ColData::Str(cn_c), ColData::Enum(rar_c)) =
        (col(2), col(20), col(14), col(5), col(7), col(8))
    else {
        return Err(bad());
    };
    let (ColData::Int(fin_c), ColData::Bool(promo_c), ColData::Bool(reprint_c), ColData::Bool(digital_c)) = (col(13), col(10), col(11), col(12)) else {
        return Err(bad());
    };
    for i in 0..st.tables[2].rows() {
        let layout = layout_c[i].and_then(|l| layouts.get(l as usize)).map(|x| x.as_str());
        let playtest = ptypes_c[i].as_ref().is_some_and(|v| v.iter().any(|&t| promo_types.get(t as usize).is_some_and(|n| n == "playtest")));
        let rar = rar_c[i].and_then(|id| rarities.get(id as usize)).map(|s| s.as_str());
        a.printing(layout, extra_c[i].as_deref(), playtest, set_c[i].as_deref(), cn_c[i].as_deref(), rar, (fin_c[i], promo_c[i], reprint_c[i], digital_c[i]));
    }
    Ok(a.finish())
}

/// The full spec the MTG dialect gives the generic index builder.
pub fn index_spec(st: &Store) -> Result<IndexSpec, String> {
    Ok(IndexSpec { derived: derive_store(st)?, skip_auto: SKIP_AUTO.iter().map(|s| s.to_string()).collect() })
}

pub struct MtgHooks;

impl Hooks for MtgHooks {
    fn required(&self) -> &[&str] {
        &["oracle.mana_f", "oracle.pt_f", "oracle.is_o", "printing.is_p"]
    }
    fn custom(&self, ix: &GIndex, name: &str, args: &[String]) -> Result<Bits, String> {
        match name {
            // args: [field ("text" | "full_text"), folded needle containing `~`]
            "tilde" => {
                let (field, q) = (&args[0], &args[1]);
                let arena = ix.text_arena(field)?;
                let names = ix.text_arena("name_plain")?;
                let parts: Vec<&str> = q.split('~').collect();
                let lit = parts.iter().max_by_key(|p| p.len()).copied().unwrap_or("");
                let cand: Vec<u32> = if lit.len() >= 3 { ix.text_find_rows(field, lit)? } else { ix.live_rows("oracle")? };
                let rows: Vec<u32> = cand
                    .into_iter()
                    .filter(|&i| {
                        let name = names.get(i as usize);
                        let mut variants: Vec<String> = vec![name.to_string()];
                        for f in name.split(" // ") {
                            variants.push(f.to_string());
                            if let Some((short, _)) = f.split_once(',') {
                                variants.push(short.to_string());
                            }
                        }
                        variants.extend(THIS_WORDS.iter().map(|w| format!("this {w}")));
                        let text = arena.get(i as usize);
                        expand_tilde(&parts, &variants).iter().any(|n| text.contains(n.as_str()))
                    })
                    .collect();
                ix.lift_rows("oracle", &rows)
            }
            "mv_parity" => {
                let odd = args[0].eq_ignore_ascii_case("odd");
                let mv = ix.num_at("cmc")?;
                ix.pred("oracle", |i| mv(i).is_some_and(|v| v.fract() == 0.0 && ((v as i64) % 2 != 0) == odd))
            }
            "name_exact" => {
                let names = ix.text_arena("name_n")?;
                let q = &args[0];
                ix.pred("oracle", |i| {
                    let n = names.get(i);
                    n == q || n.split(" // ").any(|f| f == q)
                })
            }
            // args: [stat (pow|tou|loy), op, rhs kind (num|stat), rhs]
            "stat" => {
                let li = stat_idx(&args[0]).ok_or("bad stat")?;
                let op = parse_op(&args[1])?;
                let pt = ix.text_arena("pt_f")?;
                let mv = ix.num_at("cmc")?;
                let num: Option<f64> = if args[2] == "num" { Some(args[3].parse().map_err(|_| "bad number".to_string())?) } else { None };
                let ri = stat_idx(&args[3]);
                let rhs_mv = args[2] == "stat" && args[3] == "mv";
                ix.pred("oracle", |i| {
                    pt.get(i).split(" // ").any(|face| {
                        let parts: Vec<&str> = face.split('/').collect();
                        let Some(l) = parts.get(li).and_then(|x| stat_num(x)) else { return false };
                        let r = if let Some(n) = num {
                            Some(n)
                        } else if rhs_mv {
                            mv(i)
                        } else {
                            ri.and_then(|k| parts.get(k)).and_then(|x| stat_num(x))
                        };
                        r.is_some_and(|r| num_cmp(l, r, op))
                    })
                })
            }
            // args: [overlay column, op, cents]; reads the `prices` overlay (row i = printing row i); no price never matches
            "price" => {
                let op = parse_op(&args[1])?;
                let cents: i64 = args[2].parse().map_err(|_| "bad amount".to_string())?;
                let st = ix.overlay("prices").ok_or("price search needs the prices overlay (attachOverlay)")?;
                let (_, t) = st.table("price").ok_or("prices overlay has no price table")?;
                let ci = st.schema.tables[0].columns.iter().position(|c| c.name == args[0]).ok_or("unknown price column")?;
                let coqc_format::table::ColData::Int(col) = &t.cols[ci] else { return Err("bad price column".into()) };
                ix.pred("printing", |i| col.get(i).copied().flatten().is_some_and(|c| num_cmp(c as f64, cents as f64, op)))
            }
            // args: [op, canonical cost]
            "mana" => {
                let op = parse_op(&args[0])?;
                let q = symbols(&args[1]);
                let arena = ix.text_arena("mana_f")?;
                ix.pred("oracle", |i| arena.get(i).split(" // ").any(|face| mana_cmp(&symbols(face), &q, op)))
            }
            // args: [mode (legal|banned|restricted), format]
            "legal" => {
                let st = ix.store();
                let t = ix.table_id("printing").ok_or("no printing table")?;
                let ci = st.schema.tables[t].columns.iter().position(|c| c.name == "legalities").ok_or("no legalities column")?;
                let ColData::Bytes(col) = &st.tables[t].cols[ci] else { return Err("legalities column type".into()) };
                let fid = tables::meta_of(st).formats.id(&args[1]).ok_or_else(|| format!("unknown format: {}", args[1]))? as usize;
                let mode = args[0].as_str();
                ix.pred("printing", |i| {
                    let v = col[i].as_deref().and_then(|b| decode_nibbles(&mut Reader::new(b)).ok().flatten()).and_then(|v| v.get(fid).copied());
                    match mode {
                        "banned" => v == Some(4),
                        "restricted" => v == Some(3),
                        _ => matches!(v, Some(1 | 3)),
                    }
                })
            }
            // args: [printing text column, folded needle]
            "pstr" => {
                let st = ix.store();
                let t = ix.table_id("printing").ok_or("no printing table")?;
                let ci = st.schema.tables[t].columns.iter().position(|c| c.name == args[0]).ok_or_else(|| format!("unknown column {}", args[0]))?;
                let ColData::Str(col) = &st.tables[t].cols[ci] else { return Err(format!("{} is not a text column", args[0])) };
                ix.pred("printing", |i| col[i].as_deref().is_some_and(|v| fold(v).contains(args[1].as_str())))
            }
            // args: [lowercase keyword]
            "keyword" => {
                let st = ix.store();
                let t = ix.table_id("oracle").ok_or("no oracle table")?;
                let ci = st.schema.tables[t].columns.iter().position(|c| c.name == "extra").ok_or("no extra column")?;
                let ColData::Json(col) = &st.tables[t].cols[ci] else { return Err("extra column type".into()) };
                ix.pred("oracle", |i| col[i].as_deref().is_some_and(|x| has_keyword(x, &args[0])))
            }
            _ => Err(format!("unknown custom predicate {name}")),
        }
    }
}

fn cmp(field: &str, op: Op, val: Val) -> GExpr {
    GExpr::Cmp { field: field.into(), op, val }
}

pub fn translate_expr(e: &Expr) -> GExpr {
    let text = |field: &str, q: &str| {
        let q = fold(q);
        if q.contains('~') {
            GExpr::Custom { name: "tilde".into(), args: vec![field.into(), q] }
        } else {
            GExpr::Text { field: field.into(), needle: q }
        }
    };
    match e {
        Expr::And(v) => GExpr::And(v.iter().map(translate_expr).collect()),
        Expr::Or(v) => GExpr::Or(v.iter().map(translate_expr).collect()),
        Expr::Not(x) => GExpr::Not(Box::new(translate_expr(x))),
        Expr::Name(q) => GExpr::Text { field: "name_n".into(), needle: fold_name(q) },
        Expr::Oracle(q) => text("text", q),
        Expr::FullOracle(q) => text("full_text", q),
        Expr::Type(q) => GExpr::Text { field: "type_f".into(), needle: fold(q) },
        Expr::MvParity(w) => GExpr::Custom { name: "mv_parity".into(), args: vec![w.clone()] },
        Expr::CnNum(m) => cmp("cn_num", m.op, Val::Num(m.value)),
        Expr::NameExact(q) => GExpr::Custom { name: "name_exact".into(), args: vec![fold_name(q)] },
        Expr::ColorCount(m) => GExpr::Count { field: "colors_m".into(), op: m.op, n: m.value },
        Expr::IdentityCount(m) => GExpr::Count { field: "identity_m".into(), op: m.op, n: m.value },
        Expr::Color(c) => cmp("colors_m", c.op, Val::Mask(mask_of(&c.colors) as u32)),
        Expr::Identity(c) => {
            // Scryfall: bare `:` on identity = subset (`id<=`)
            let op = if c.op == Op::Colon { Op::Le } else { c.op };
            cmp("identity_m", op, Val::Mask(mask_of(&c.colors) as u32))
        }
        Expr::Mv(m) => cmp("cmc", m.op, Val::Num(m.value)),
        Expr::Rarity(r) => {
            let want = r.value.to_ascii_lowercase();
            match rarity_rank(&want) {
                Some(w) => {
                    let base = cmp("rarity_rank", r.op, Val::Num(w as f64));
                    if r.op == Op::Ne {
                        // a rarity name outside the known six matches `!=` only
                        GExpr::Or(vec![
                            base,
                            GExpr::And(vec![
                                GExpr::Cmp { field: "rarity_s".into(), op: Op::Ne, val: Val::Str(String::new()) },
                                GExpr::Not(Box::new(GExpr::Present { field: "rarity_rank".into() })),
                            ]),
                        ])
                    } else {
                        base
                    }
                }
                None => {
                    let op = if r.op == Op::Ne { Op::Ne } else { Op::Eq };
                    GExpr::And(vec![GExpr::Cmp { field: "rarity_s".into(), op: Op::Ne, val: Val::Str(String::new()) }, cmp("rarity_s", op, Val::Str(want))])
                }
            }
        }
        Expr::Set(s) => cmp("set_f", Op::Eq, Val::Str(fold(s))),
        Expr::Cn(c) => cmp("cn_f", Op::Eq, Val::Str(fold(c))),
        Expr::Regex { field, pattern } => {
            let f = match field.as_str() {
                "name" => "name_plain",
                "type" => "type_f",
                "fulloracle" => "full_text",
                _ => "text",
            };
            GExpr::Regex { field: f.into(), pattern: pattern.clone() }
        }
        Expr::Stat { stat, op, rhs } => {
            let (kind, r) = match rhs {
                StatRhs::Num(n) => ("num", n.to_string()),
                StatRhs::Stat(s) => ("stat", s.clone()),
            };
            GExpr::Custom { name: "stat".into(), args: vec![stat.clone(), op_sym(*op).into(), kind.into(), r] }
        }
        Expr::Price { col, op, cents } => GExpr::Custom { name: "price".into(), args: vec![col.clone(), op_sym(*op).into(), cents.to_string()] },
        Expr::Mana { op, cost } => GExpr::Custom { name: "mana".into(), args: vec![op_sym(*op).into(), cost.clone()] },
        Expr::Legal { mode, format } => GExpr::Custom { name: "legal".into(), args: vec![mode.clone(), format.clone()] },
        Expr::Keyword(k) => GExpr::Custom { name: "keyword".into(), args: vec![k.clone()] },
        Expr::Artist(a) => GExpr::Custom { name: "pstr".into(), args: vec!["artist".into(), fold(a)] },
        Expr::Flavor(a) => GExpr::Custom { name: "pstr".into(), args: vec!["flavor_text".into(), fold(a)] },
        Expr::Is(n) => match IS_O.iter().position(|x| x == n) {
            Some(i) => cmp("is_o", Op::Ge, Val::Mask(1 << i)),
            None => cmp("is_p", Op::Ge, Val::Mask(1 << IS_P.iter().position(|x| x == n).unwrap_or(0))),
        },
    }
}

pub fn translate(q: &Query) -> GQuery {
    let default = [SortKey { field: SortField::Released, desc: true }, SortKey { field: SortField::Name, desc: false }];
    let keys: &[SortKey] = if q.sort.is_empty() { &default } else { &q.sort };
    GQuery {
        expr: q.expr.as_ref().map(translate_expr),
        sort: keys
            .iter()
            .map(|k| {
                let (field, null) = match k.field {
                    SortField::Released => ("released_at", 0.0),
                    SortField::Name => ("name_sort", 0.0),
                    SortField::Mv => ("cmc", 0.0),
                    SortField::Rarity => ("rarity_rank", -1.0),
                };
                GSort { field: field.into(), desc: k.desc, null }
            })
            .collect(),
        unique_parent: q.unique == Unique::Cards,
        prefer: Some(GPrefer { field: "released_at".into(), newest: q.prefer == Prefer::Newest }),
        hide: if q.include_extras { None } else { Some("hidden".into()) },
        offset: q.offset,
        limit: q.limit,
        scope: q.scope.clone(),
    }
}
