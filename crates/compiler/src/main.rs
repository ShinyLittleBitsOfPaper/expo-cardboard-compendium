use coqc::{compile, synth, verify, Options, RowOrder};
use coqc_format::container::{block_table, parse};
use coqc_format::{read, write, SectionId};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Instant;

const USAGE: &str = "\
coqc - compile Scryfall bulk card data into a COQD dataset

USAGE:
  coqc compile <input.jsonl|json> -o <out.coqd> [--prev <prev.coqd>] [--rebase]
               [--source-date YYYYMMDD] [--keep-prices] [--progress] [--breakdown]
               [--row-order set|uuid]   (default set: new rows grouped by set, then collector number)
  coqc verify  <dataset.coqd> <input.jsonl|json>   lossless round-trip check
  coqc inspect <dataset.coqd>                      sizes, dictionaries, samples
  coqc diff    <old.coqd> <new.coqd> [--keys]      blocks changed between releases;
                                                   --keys: which fields changed, per row
  coqc patch   <old.coqd> <new.coqd> -o <out.coqp>  record-level patch (same epoch)
  coqc apply   <old.coqd> <patch.coqp> -o <out.coqd> apply it; verifies the result's SHA-256
  coqc search  <dataset.coqd> <query words...> [--plan '<json Query>'] [--cards] [--limit N] [--offset N]
                                                   run a query (quick syntax: t:instant c>=r mv<=2 -o:flying \"bolt\")
  coqc index-stats <dataset.coqd>                  derived search index: build time, sizes, stop-grams
  coqc idmap   <AllIdentifiers.json> -o <idmap.coqm>   MTGJSON uuid -> Scryfall id map (refresh rarely)
  coqc prices  <AllPricesToday.json> --cards <cards.coqd> --idmap <idmap.coqm> -o <prices.coqr>
  coqc prices-patch <old.coqr> <new.coqr> -o <p.coqx>  /  coqc prices-apply <old.coqr> <p.coqx> -o <new.coqr>
  coqc price   <prices.coqr> <cards.coqd> <card name>  show stored prices of every printing of a card
  coqc synth   <n_printings> <out.jsonl> [--day N] synthetic data for scale tests
";

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn mb(n: u64) -> String {
    format!("{:.2} MB", n as f64 / 1_048_576.0)
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn load(path: &Path) -> (Vec<u8>, coqc_format::Dataset) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    let ds = read(&bytes).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    (bytes, ds)
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn cmd_compile(args: &[String]) {
    let input = args.first().filter(|a| !a.starts_with('-')).unwrap_or_else(|| die("missing input file"));
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqd>"));
    let opts = Options {
        rebase: args.iter().any(|a| a == "--rebase"),
        keep_prices: args.iter().any(|a| a == "--keep-prices"),
        skip_digital: args.iter().any(|a| a == "--skip-digital"),
        skip_art_series: args.iter().any(|a| a == "--skip-art-series"),
        progress: args.iter().any(|a| a == "--progress"),
        row_order: match flag_value(args, "--row-order").as_deref() {
            None | Some("set") => RowOrder::Set,
            Some("uuid") => RowOrder::Uuid,
            Some(o) => die(&format!("unknown --row-order {o} (use set or uuid)")),
        },
        source_date: flag_value(args, "--source-date")
            .map_or(0, |s| s.parse().unwrap_or_else(|_| die("bad --source-date"))),
    };
    let prev = flag_value(args, "--prev").map(|p| load(Path::new(&p)).1);

    let t = Instant::now();
    let (ds, st) = compile(Path::new(input), prev.as_ref(), &opts).unwrap_or_else(|e| die(&e));
    let t_compile = t.elapsed();
    if opts.progress {
        eprintln!("writing dataset (zstd level {}), this takes a while...", coqc_format::container::ZSTD_LEVEL);
    }
    let t = Instant::now();
    let (bytes, sections) = write(&ds).unwrap_or_else(|e| die(&e.to_string()));
    let t_write = t.elapsed();
    std::fs::write(&out, &bytes).unwrap_or_else(|e| die(&format!("{out}: {e}")));
    let sha = sha_hex(&bytes);
    let fname = PathBuf::from(&out).file_name().unwrap().to_string_lossy().to_string();
    std::fs::write(format!("{out}.sha256"), format!("{sha}  {fname}\n")).ok();

    println!("input lines/cards : {} ({} unparseable)", st.lines, st.parse_errors);
    println!(
        "skipped           : digital {}, art_series {}, no oracle_id {}, bad id {}, duplicate id {}",
        st.skipped_digital, st.skipped_art_series, st.skipped_no_oracle_id, st.skipped_bad_id, st.duplicate_ids
    );
    println!("oracle cards      : {} ({} new, {} tombstoned)", st.oracle_cards, st.new_oracle, st.tombstoned_oracle);
    println!("printings         : {} ({} new, {} tombstoned)", st.printings, st.new_printings, st.tombstoned_printings);
    println!("images            : {} canonical, {} raw", st.images_canonical, st.images_raw);
    for (k, (hit, miss)) in &st.derived {
        println!("derived key       : {k}: {hit} derived (not stored), {miss} stored because different");
    }
    println!("healed printings  : {} (typed form differed; kept via extra)", st.healed_printings);
    let mut healed: Vec<_> = st.healed_keys.iter().collect();
    healed.sort_by(|a, b| b.1.cmp(a.1));
    for (k, v) in healed.iter().take(25) {
        let ex = st.healed_examples.get(*k).map(String::as_str).unwrap_or("");
        println!("    {k:<22} {v:>6}   e.g. {ex}");
    }
    let mut why: Vec<_> = st.image_mismatches.iter().collect();
    why.sort_by(|a, b| b.1.cmp(a.1));
    for (sig, n) in why.iter().take(15) {
        println!("    image not canonical x{n}: {sig}");
    }
    let mut extras: Vec<_> = st.extra_bytes_by_key.iter().collect();
    extras.sort_by(|a, b| b.1.cmp(a.1));
    println!("largest residual (`extra`) keys, raw JSON bytes:");
    for (k, v) in extras.iter().take(12) {
        println!("    {k:<22} {}", mb(**v));
        if let Some(e) = st.extra_examples.get(*k) {
            println!("        e.g. {e}");
        }
    }
    println!("sections:");
    for s in &sections {
        println!("    {:<15} raw {:>11}  stored {:>11}  blocks {}", s.name, mb(s.raw_bytes), mb(s.stored_bytes), s.blocks);
    }
    if args.iter().any(|a| a == "--breakdown") {
        if opts.progress {
            eprintln!("computing per-key compressed sizes...");
        }
        let b = coqc::breakdown(&ds).unwrap_or_else(|e| die(&e));
        let printing_stored = sections.iter().find(|s| s.name == "printing").map_or(1, |s| s.stored_bytes);
        let mut rows: Vec<_> = b.iter().collect();
        rows.sort_by(|a, b| b.1.stored.cmp(&a.1.stored));
        println!("estimated stored size per residual key (printing section stored {}):", mb(printing_stored));
        let mut sum = 0u64;
        for (k, st) in rows.iter().take(25) {
            println!("    {k:<24} raw {:>10}  stored {:>10}  {:>5.1}%", mb(st.raw), mb(st.stored), 100.0 * st.stored as f64 / printing_stored as f64);
        }
        for (_, st) in &rows {
            sum += st.stored;
        }
        println!("    (all {} residual keys: {}; everything else {} = typed fields, images, structure; estimates)", rows.len(), mb(sum), mb(printing_stored.saturating_sub(sum)));
        println!("keys identical across all printings, among oracle cards with 2+ printings having the key (could be stored once on the oracle record):");
        for (k, st) in rows.iter().filter(|(_, s)| s.oracles_with > 0) {
            println!("    {k:<24} {:>6} of {:>6} identical", st.oracles_with - st.oracles_varied, st.oracles_with);
        }
    }
    println!("epoch {}  file {}  sha256 {sha}", ds.meta.cards_epoch, mb(bytes.len() as u64));
    println!("time: compile {:.2?}, write {:.2?}", t_compile, t_write);
}

fn cmd_verify(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc verify <dataset.coqd> <input>");
    }
    let (_, ds) = load(Path::new(&args[0]));
    let t = Instant::now();
    let (checked, bad) = verify(&ds, Path::new(&args[1])).unwrap_or_else(|e| die(&e));
    println!("checked {checked} cards, {bad} mismatches ({:.2?})", t.elapsed());
    if bad > 0 {
        exit(2);
    }
}

fn cmd_inspect(args: &[String]) {
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let (bytes, ds) = load(Path::new(path));
    let p = parse(&bytes).unwrap_or_else(|e| die(&e.to_string()));
    println!(
        "epoch {}  source_date {}  oracle rows {}  printing rows {}",
        p.cards_epoch, p.source_date, p.oracle_count, p.printing_count
    );
    println!("file {}  sha256 {}", mb(bytes.len() as u64), sha_hex(&bytes));
    for (id, sec) in &p.sections {
        println!("  section {id}: {}", mb(sec.len() as u64));
    }
    let m = &ds.meta;
    for (n, d) in [
        ("layouts", &m.layouts),
        ("rarities", &m.rarities),
        ("border_colors", &m.border_colors),
        ("langs", &m.langs),
        ("promo_types", &m.promo_types),
        ("image_keys", &m.image_keys),
        ("formats", &m.formats),
    ] {
        println!("  dict {n}: {} entries", d.len());
    }
    println!("  dropped keys: {:?}", m.dropped_keys);
    println!(
        "  tombstones: {} oracle, {} printings",
        ds.oracle.iter().filter(|r| r.tombstone).count(),
        ds.printings.iter().filter(|r| r.tombstone).count()
    );
    if let Some(po) = ds.printings.iter().find(|r| !r.tombstone) {
        let card = coqc_format::json::card_json(m, &ds.oracle[po.oracle_row as usize], &ds.ranks[po.oracle_row as usize], po);
        println!("sample card (rebuilt):\n{}", serde_json::to_string_pretty(&card).unwrap());
    }
}

fn cmd_diff(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc diff <old> <new>");
    }
    let (a, _) = load(Path::new(&args[0]));
    let (b, _) = load(Path::new(&args[1]));
    let mut total_patch = 0u64;
    for id in [SectionId::Oracle, SectionId::Printing] {
        let (ta, tb) = (block_table(&a, id).unwrap(), block_table(&b, id).unwrap());
        let mut changed = 0;
        let mut bytes = 0u64;
        for (i, blk) in tb.iter().enumerate() {
            if ta.get(i).map(|x| x.hash) != Some(blk.hash) {
                changed += 1;
                bytes += blk.clen as u64;
            }
        }
        println!(
            "{:<9} blocks {} -> {}  changed/new {} ({:.1}%)  changed bytes {}",
            id.name(),
            ta.len(),
            tb.len(),
            changed,
            100.0 * changed as f64 / tb.len().max(1) as f64,
            mb(bytes)
        );
        total_patch += bytes;
    }
    println!("naive block patch (uuid indexes rebuilt client-side): {}", mb(total_patch));
    if args.iter().any(|a| a == "--keys") {
        let (da, db) = (read(&a).unwrap(), read(&b).unwrap());
        let (o, p, r) = coqc::diff_keys(&da, &db);
        println!("oracle ranks table (edhrec/penny): {r} rows changed");
        for (name, d) in [("oracle", &o), ("printing", &p)] {
            println!(
                "{name}: rows {} -> {} ({} appended), {} existing rows changed, {} newly tombstoned, {} revived",
                d.rows_old, d.rows_new, d.rows_new - d.rows_old, d.rows_changed, d.became_tombstone, d.revived
            );
            let mut keys: Vec<_> = d.by_key.iter().collect();
            keys.sort_by(|a, b| b.1.cmp(a.1));
            for (k, n) in keys.iter().take(20) {
                println!("    {k:<30} changed on {n} rows");
            }
        }
    }
}

fn cmd_patch(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc patch <old.coqd> <new.coqd> -o <out.coqp>");
    }
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqp>"));
    let (a, b) = (std::fs::read(&args[0]).unwrap_or_else(|e| die(&e.to_string())), std::fs::read(&args[1]).unwrap_or_else(|e| die(&e.to_string())));
    let (patch, i) = coqc::patch::make_patch(&a, &b).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, &patch).unwrap_or_else(|e| die(&e.to_string()));
    println!(
        "patch {}: {} (raw {}) vs full file {}\n  oracle rows: {} changed, {} appended; printing rows: {} changed, {} appended",
        out, mb(i.patch_bytes), mb(i.raw_bytes), mb(b.len() as u64), i.oracle_changed, i.oracle_appended, i.printing_changed, i.printing_appended
    );
}

fn cmd_apply(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc apply <old.coqd> <patch.coqp> -o <out.coqd>");
    }
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqd>"));
    let (a, p) = (std::fs::read(&args[0]).unwrap_or_else(|e| die(&e.to_string())), std::fs::read(&args[1]).unwrap_or_else(|e| die(&e.to_string())));
    let t = Instant::now();
    let new = coqc::patch::apply_patch(&a, &p).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, &new).unwrap_or_else(|e| die(&e.to_string()));
    println!("applied: {} ({}), sha256 {} verified ({:.2?})", out, mb(new.len() as u64), sha_hex(&new), t.elapsed());
}

fn cmd_synth(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc synth <n> <out.jsonl> [--day N]");
    }
    let n: u64 = args[0].parse().unwrap_or_else(|_| die("bad n"));
    let day: u64 = flag_value(args, "--day").map_or(1, |d| d.parse().unwrap_or_else(|_| die("bad --day")));
    let w = synth::generate(Path::new(&args[1]), n, day).unwrap_or_else(|e| die(&e.to_string()));
    let size = std::fs::metadata(&args[1]).map(|m| m.len()).unwrap_or(0);
    println!("wrote {w} synthetic cards ({}) to {}", mb(size), args[1]);
}

fn cmd_index_stats(args: &[String]) {
    use coqc_format::search::SearchIndex;
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let (_, ds) = load(Path::new(path));
    let t = Instant::now();
    let ix = SearchIndex::build(&ds);
    let ms = t.elapsed().as_millis();
    let s = ix.stats();
    println!("index build: {ms} ms (single thread, this machine)");
    println!("oracle rows {}", s.oracle_rows);
    println!("names : text {}  grams {}  stop {}  index ~{}", mb(s.name_bytes as u64), s.name_grams, s.name_stop, mb(s.name_index_bytes as u64));
    println!("text  : text {}  grams {}  stop {}  index ~{}", mb(s.text_bytes as u64), s.text_grams, s.text_stop, mb(s.text_index_bytes as u64));
    for q in ["bolt", "t:creature", "o:flying", "o:\"draw a card\"", "o:\"enters the battlefield\"", "o:ab", "c>=rg mv<=2", "o:destroy t:artifact"] {
        let expr = coqc::quick::parse(q).unwrap_or_else(|e| die(&e));
        let query = coqc_format::search::Query { expr, ..Default::default() };
        let t = Instant::now();
        let r = ix.search(&query);
        println!("  {q:<32} {:>7} prints  {:>6.1} ms", r.total, t.elapsed().as_secs_f64() * 1000.0);
    }
}

fn cmd_search(args: &[String]) {
    use coqc_format::search::{Query, SearchIndex, Unique};
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let (_, ds) = load(Path::new(path));
    let mut query: Query = match flag_value(args, "--plan") {
        Some(j) => serde_json::from_str(&j).unwrap_or_else(|e| die(&format!("bad --plan: {e}"))),
        None => {
            let mut words = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--cards" => {}
                    "--limit" | "--offset" | "--plan" => i += 1,
                    w => words.push(w.to_string()),
                }
                i += 1;
            }
            Query { expr: coqc::quick::parse(&words.join(" ")).unwrap_or_else(|e| die(&e)), ..Default::default() }
        }
    };
    if args.iter().any(|a| a == "--cards") {
        query.unique = Unique::Cards;
    }
    query.limit = Some(flag_value(args, "--limit").map_or(20, |v| v.parse().unwrap_or_else(|_| die("bad --limit"))));
    query.offset = flag_value(args, "--offset").map_or(0, |v| v.parse().unwrap_or_else(|_| die("bad --offset")));
    let t = Instant::now();
    let ix = SearchIndex::build(&ds);
    let built = t.elapsed();
    let t = Instant::now();
    let r = ix.search(&query);
    let took = t.elapsed();
    println!("{} matches (index {} ms, query {:.2} ms)", r.total, built.as_millis(), took.as_secs_f64() * 1000.0);
    for row in r.rows {
        let p = &ds.printings[row as usize];
        println!(
            "  {:>6}  {} {:<5} {:<5} {}",
            row,
            p.released_at.map_or("-".into(), |d| coqc_format::json::released_to_string(d)),
            p.set_code.as_deref().unwrap_or("-"),
            p.collector_number.as_deref().unwrap_or("-"),
            ds.oracle[p.oracle_row as usize].name.as_deref().unwrap_or("?")
        );
    }
}

fn rd(p: &str) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| die(&format!("{p}: {e}")))
}

fn cmd_idmap(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <idmap.coqm>"));
    let (b, n) = coqc::prices::build_idmap(&rd(args.first().unwrap_or_else(|| die("missing input")))).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, &b).unwrap_or_else(|e| die(&e.to_string()));
    println!("idmap {out}: {n} uuids, {}", mb(b.len() as u64));
}

fn cmd_prices(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <prices.coqr>"));
    let cards = flag_value(args, "--cards").unwrap_or_else(|| die("missing --cards"));
    let idm = flag_value(args, "--idmap").unwrap_or_else(|| die("missing --idmap"));
    let t0 = Instant::now();
    let (_, ds) = load(Path::new(&cards));
    let idmap = coqc::prices::load_idmap(&rd(&idm)).unwrap_or_else(|e| die(&e.to_string()));
    let (t, r) = coqc::prices::compile_prices(&ds, &idmap, &rd(args.first().unwrap_or_else(|| die("missing input")))).unwrap_or_else(|e| die(&e));
    let b = t.encode().unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(&out, &b).unwrap_or_else(|e| die(&e.to_string()));
    println!("date {}  entries {}  unmapped uuids {}  not in cards file {}  conflicts {}", r.date, r.entries, r.unmapped, r.not_in_cards, r.conflicts);
    println!("rows priced {} of {}", r.rows_priced, t.rows());
    for (n, c) in coqc_format::prices::COLUMNS.iter().zip(r.per_column) {
        println!("  {n:<11} {c}");
    }
    println!("prices {out}: {} ({:.1}s)", mb(b.len() as u64), t0.elapsed().as_secs_f64());
}

fn cmd_prices_patch(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <p.coqx>"));
    if args.len() < 2 {
        die("usage: coqc prices-patch <old.coqr> <new.coqr> -o <p.coqx>");
    }
    let (a, b) = (rd(&args[0]), rd(&args[1]));
    let p = coqc_format::prices::make_delta(&a, &b).unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(&out, &p).unwrap_or_else(|e| die(&e.to_string()));
    println!("price delta {out}: {} vs full file {}", mb(p.len() as u64), mb(b.len() as u64));
}

fn cmd_prices_apply(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <new.coqr>"));
    if args.len() < 2 {
        die("usage: coqc prices-apply <old.coqr> <p.coqx> -o <new.coqr>");
    }
    let n = coqc_format::prices::apply_delta(&rd(&args[0]), &rd(&args[1])).unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(&out, &n).unwrap_or_else(|e| die(&e.to_string()));
    println!("applied: {out} ({}), hash verified", mb(n.len() as u64));
}

fn cmd_price(args: &[String]) {
    if args.len() < 3 {
        die("usage: coqc price <prices.coqr> <cards.coqd> <card name>");
    }
    let t = coqc_format::prices::PriceTable::decode(&rd(&args[0])).unwrap_or_else(|e| die(&e.to_string()));
    let (_, ds) = load(Path::new(&args[1]));
    let want = args[2..].join(" ").to_lowercase();
    println!("prices dated {}", t.date);
    for (i, p) in ds.printings.iter().enumerate() {
        if p.tombstone || ds.oracle[p.oracle_row as usize].name.as_deref().map(str::to_lowercase).as_deref() != Some(&want) {
            continue;
        }
        let f = |c: usize, sym: &str| t.cents(i, c).map_or("-".to_string(), |v| format!("{sym}{}.{:02}", v / 100, v % 100));
        println!(
            "  row {i:>6} {:<6} {:<5} tcg {} / {} / {}   cm {} / {} / {}",
            p.set_code.as_deref().unwrap_or("-"), p.collector_number.as_deref().unwrap_or("-"),
            f(0, "$"), f(1, "$"), f(2, "$"), f(3, "€"), f(4, "€"), f(5, "€")
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        exit(1)
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "compile" => cmd_compile(rest),
        "verify" => cmd_verify(rest),
        "inspect" => cmd_inspect(rest),
        "diff" => cmd_diff(rest),
        "synth" => cmd_synth(rest),
        "idmap" => cmd_idmap(rest),
        "prices" => cmd_prices(rest),
        "prices-patch" => cmd_prices_patch(rest),
        "prices-apply" => cmd_prices_apply(rest),
        "price" => cmd_price(rest),
        "search" => cmd_search(rest),
        "index-stats" => cmd_index_stats(rest),
        "patch" => cmd_patch(rest),
        "apply" => cmd_apply(rest),
        _ => {
            print!("{USAGE}");
            exit(1)
        }
    }
}
