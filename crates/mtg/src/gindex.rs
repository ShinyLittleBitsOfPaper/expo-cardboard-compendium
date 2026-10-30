//! MTG side of the generic index (S3a): derived columns, `Custom` predicates, and the translation of the Scryfall query tree
//! (`coqc_format::search::Query`) into the generic one. Derivation still reads the legacy `Dataset` (S3b/S3c move it onto the
//! `Store`); the folding rules are exactly those of `SearchIndex::build`.
use coqc_format::gindex::*;
use coqc_format::search::*;
use coqc_format::Dataset;

pub const TEXT_GROUP: &str = "text";

pub fn derive(ds: &Dataset) -> Vec<DerivedCol> {
    let (mut name_plain, mut name_n, mut name_sort, mut type_f, mut text, mut full) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    let (mut colors, mut identity) = (vec![], vec![]);
    for o in &ds.oracle {
        let name = o.name.clone().unwrap_or_default();
        name_plain.push(fold(&name));
        name_n.push(fold_name(&name));
        name_sort.push(fold(&name).chars().filter(|c| c.is_alphanumeric()).collect::<String>());
        type_f.push(fold(&join_faces(&o.type_line, &o.faces, |f| &f.type_line)));
        let f = join_faces(&o.oracle_text, &o.faces, |f| &f.oracle_text);
        text.push(fold(&strip_reminder(&f)));
        full.push(fold(&f));
        colors.push(oracle_mask(o, o.colors, "colors", true) as u32);
        identity.push(oracle_mask(o, o.color_identity, "color_identity", false) as u32);
    }
    let rarities: Vec<String> = ds.meta.rarities.values().iter().map(|r| r.to_string()).collect();
    let layouts = ds.meta.layouts.values();
    let promo_types = ds.meta.promo_types.values();
    let (mut set_f, mut cn_f, mut hidden, mut rank, mut rar_s, mut cn_num) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    for p in &ds.printings {
        let layout = p.layout.and_then(|l| layouts.get(l as usize)).map(|x| x.as_str());
        let set_type = p.extra.as_deref().and_then(json_str_field_set_type);
        let playtest = p.promo_types.as_ref().is_some_and(|v| v.iter().any(|&t| promo_types.get(t as usize).is_some_and(|n| n == "playtest")));
        hidden.push(matches!(layout, Some("vanguard" | "planar" | "scheme" | "phenomenon" | "art_series")) || matches!(set_type, Some("memorabilia")) || playtest);
        set_f.push(p.set_code.as_deref().map(fold).unwrap_or_default());
        let cn = p.collector_number.as_deref().map(fold).unwrap_or_default();
        let d: String = cn.chars().take_while(|c| c.is_ascii_digit()).collect();
        cn_num.push(d.parse::<f64>().ok());
        cn_f.push(cn);
        let name = p.rarity.and_then(|id| rarities.get(id as usize));
        rar_s.push(name.cloned().unwrap_or_default());
        rank.push(name.and_then(|n| rarity_rank(n)).map(|r| r as f64));
    }
    let t = |table: &str, name: &str, v: Vec<String>, g: Option<&str>| DerivedCol { table: table.into(), name: name.into(), data: DData::Text(v), tri_group: g.map(String::from) };
    vec![
        t("oracle", "name_plain", name_plain, None),
        t("oracle", "name_n", name_n, Some("name")),
        t("oracle", "name_sort", name_sort, None),
        t("oracle", "type_f", type_f, None),
        t("oracle", "text", text, Some(TEXT_GROUP)),
        t("oracle", "full_text", full, Some(TEXT_GROUP)),
        DerivedCol { table: "oracle".into(), name: "colors_m".into(), data: DData::Mask(colors), tri_group: None },
        DerivedCol { table: "oracle".into(), name: "identity_m".into(), data: DData::Mask(identity), tri_group: None },
        t("printing", "set_f", set_f, None),
        t("printing", "cn_f", cn_f, None),
        t("printing", "rarity_s", rar_s, None),
        DerivedCol { table: "printing".into(), name: "hidden".into(), data: DData::Flag(hidden), tri_group: None },
        DerivedCol { table: "printing".into(), name: "rarity_rank".into(), data: DData::Num(rank), tri_group: None },
        DerivedCol { table: "printing".into(), name: "cn_num".into(), data: DData::Num(cn_num), tri_group: None },
    ]
}

pub struct MtgHooks;

impl Hooks for MtgHooks {
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
