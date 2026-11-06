use coqc::{compile, synth, verify, Options, RowOrder};
use coqc_format::coqi;
use coqc_format::coqd::{block_table, parse};
use coqc_mtg::{read, write};
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
  coqc index-build <dataset.coqd> -o <out.coqi>    serialize the search index (compile-time artifact); sizes + load check
  coqc index-diff  <old> <new> [-o <delta.coqid>]  index delta between two releases (.coqd or .coqi); replay-verified
  coqc index-apply <old.coqi> <delta.coqid> -o <new.coqi>   replay a delta (hash-verified)
  coqc idmap   <AllIdentifiers.json> -o <idmap.coqm>   MTGJSON uuid -> Scryfall id map (refresh rarely)
  coqc prices  <AllPricesToday.json> --cards <cards.coqd> --idmap <idmap.coqm> -o <prices.coqd>
  coqc prices-patch <old.coqd> <new.coqd> -o <p.coqp>  /  coqc prices-apply <old.coqd> <p.coqp> -o <new.coqd>
  coqc price   <prices.coqd> <cards.coqd> <card name>  show stored prices of every printing of a card
  coqc zst     <in> -o <out> [--level N]           zstd-compress a file (the published index is .coqi.zst, level 19)
  coqc unzst   <in.zst> -o <out>                   decompress it
  coqc manifest --base ID=f.coqd[,patch=f.coqp][,prefix=P-] [--base ...] [--index ID=f.coqi.zst[,of=BASE][,patch=f.coqid][,prefix=P-]]
               [--overlay ID=f.coqd[,of=BASE][,patch=f.coqp][,prefix=P-] ...] --run <data-tag> --at <ISO UTC> --keep-since <ISO UTC>
               [--repo OWNER/REPO | --base-url URL] [--prev manifest.json] [--rebase] -o manifest.json   (manifest v2, docs/manifest.md)
  coqc generic-compile <schema.json> --table NAME=file.jsonl ... -o <out.coqd> [--prev p.coqd] [--rebase] [--source-date N]
                                                   any schema-described dataset (docs/generic.md); every other command works on it too
  coqc generic-overlay <overlay-schema.json> <base.coqd> <in.jsonl> -o <out.coqd>   columnar overlay whose row i follows base row i
  coqc demo-books <dir> [--works N] [--day D]      the synthetic books dataset (schema + JSONL) used by the S5 proof
  coqc search ... [--overlay id=file.coqd]         attach overlays so queries can use their columns
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

fn load(path: &Path) -> (Vec<u8>, coqc_mtg::Dataset) {
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
        eprintln!("writing dataset (zstd level {}), this takes a while...", coqc_format::coqd::ZSTD_LEVEL);
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
        ds.meta.cards_epoch, ds.meta.source_date, ds.oracle.len(), ds.printings.len()
    );
    println!("file {}  sha256 {}", mb(bytes.len() as u64), sha_hex(&bytes));
    let schema = p.schema().unwrap_or_else(|e| die(&e.to_string()));
    for (kind, arg, sec) in &p.sections {
        let t = schema.tables.get(*arg as usize).map_or("", |t| t.name.as_str());
        let name = match kind {
            1 => "schema".to_string(),
            2 => "meta".to_string(),
            3 => format!("rows {t}"),
            _ => format!("keys {t}"),
        };
        println!("  section {name}: {}", mb(sec.len() as u64));
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
        let card = coqc_mtg::json::card_json(m, &ds.oracle[po.oracle_row as usize], &ds.ranks[po.oracle_row as usize], po);
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
    for id in ["oracle", "printing"] {
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
            id,
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
    println!("patch {}: {} (raw {}) vs full file {}", out, mb(i.patch_bytes), mb(i.raw_bytes), mb(b.len() as u64));
    for (t, changed, appended) in &i.tables {
        println!("  {t} rows: {changed} changed, {appended} appended");
    }
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

fn cmd_phash_eval(args: &[String]) {
    let dir = flag_value(args, "--images").unwrap_or_else(|| die("usage: coqc phash-eval --images DIR [--limit N]"));
    let limit: usize = flag_value(args, "--limit").map_or(usize::MAX, |d| d.parse().unwrap_or_else(|_| die("bad --limit")));
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap_or_else(|e| die(&e.to_string())).filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()).as_deref(), Some("jpg" | "jpeg" | "png"))).collect();
    paths.sort();
    paths.truncate(limit);
    let mut images = Vec::new();
    for p in &paths {
        match coqc_phash::load_gray(p) {
            Ok(g) => images.push(g),
            Err(e) => eprintln!("skip: {e}"),
        }
    }
    if images.len() < 2 {
        die("need at least 2 decodable images");
    }
    println!("{} images x {} perturbations", images.len(), coqc_phash::ALL_PERTURBS.len());
    for r in coqc_phash::evaluate(&images) {
        println!("\n== {} ==", r.name);
        println!("same-art hit rate by max Hamming distance:");
        for t in (0..=16).step_by(1) {
            println!("  <= {t:2}: {:5.1}%", r.hits[t] * 100.0);
        }
        println!("mean distance per perturbation:");
        for (p, d) in &r.per_perturb {
            println!("  {p:?}: {d:.2}");
        }
        let mut n = r.nearest_other.clone();
        n.sort();
        let pct = |q: f64| n[((n.len() - 1) as f64 * q) as usize];
        println!("nearest DIFFERENT image: min {} p5 {} p50 {} (a threshold must stay below the low end)", n[0], pct(0.05), pct(0.5));
        let close: Vec<_> = r.nearest_other.iter().enumerate().filter(|(_, d)| **d <= 8).map(|(i, d)| format!("{} (d={d})", paths[i].file_name().unwrap().to_string_lossy())).collect();
        if !close.is_empty() {
            println!("images with a different-image neighbour within 8 bits: {}", close.join(", "));
        }
    }
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

/// Load a `.coqd` as a store (the index and search commands never need the legacy records).
fn load_store(path: &Path) -> (Vec<u8>, coqc_format::table::Store) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    let st = coqc_format::read_store(&bytes).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    (bytes, st)
}

fn build_index_parts(st: &coqc_format::table::Store) -> coqc_format::gindex::IndexParts {
    let spec = coqc::dialect_for(st).index_spec(st).unwrap_or_else(|e| die(&e));
    coqc_format::gindex::GIndex::build_parts(st, spec, coqc_format::search::STOP_FRACTION, coqc_format::search::STOP_MIN).unwrap_or_else(|e| die(&e))
}

fn run_gquery(st: &coqc_format::table::Store, parts: &coqc_format::gindex::IndexParts, q: &coqc_format::gindex::GQuery) -> coqc_format::gindex::GResult {
    run_gquery_ov(st, parts, q, vec![])
}

fn run_gquery_ov<'a>(st: &'a coqc_format::table::Store, parts: &'a coqc_format::gindex::IndexParts, q: &coqc_format::gindex::GQuery, ov: Vec<(String, &'a coqc_format::table::Store)>) -> coqc_format::gindex::GResult {
    let d = coqc::dialect_for(st);
    let ix = coqc_format::gindex::GIndex::with_parts(st, Some(d.hooks()), parts).unwrap_or_else(|e| die(&e)).with_overlays(ov);
    ix.search(q).unwrap_or_else(|e| die(&e))
}

const SAMPLE_QUERIES: [&str; 8] = ["bolt", "t:creature", "o:flying", "o:\"draw a card\"", "o:\"enters the battlefield\"", "o:ab", "c>=rg mv<=2", "o:destroy t:artifact"];

fn cmd_index_stats(args: &[String]) {
    use coqc_format::dialect::Dialect;
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let (_, st) = load_store(Path::new(path));
    let t = Instant::now();
    let parts = build_index_parts(&st);
    println!("index build: {} ms (single thread, this machine)", t.elapsed().as_millis());
    for s in coqi::section_sizes(&parts, &st.schema) {
        println!("  {:<24} raw {:>10}  zstd {:>10}", s.name, mb(s.raw as u64), mb(s.zstd as u64));
    }
    let samples: &[&str] = if st.schema.id == "mtg" { &SAMPLE_QUERIES } else { &[] };
    for q in samples {
        let g = coqc_mtg::Mtg.parse_gquery(q, &st, &[]).unwrap_or_else(|e| die(&e));
        let t = Instant::now();
        let r = run_gquery(&st, &parts, &g);
        println!("  {q:<32} {:>7} prints  {:>6.1} ms", r.total, t.elapsed().as_secs_f64() * 1000.0);
    }
}

fn index_file(path: &str) -> Vec<u8> {
    // returns the .coqi bytes: read as-is, or built from a .coqd
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    if bytes.starts_with(b"COQI") {
        return bytes;
    }
    coqc::build_index_file(&bytes).unwrap_or_else(|e| die(&format!("{path}: {e}")))
}

fn cmd_index_build(args: &[String]) {
    use coqc_format::dialect::Dialect;
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqi>"));
    let (bytes, st) = load_store(Path::new(path));
    let t = Instant::now();
    let parts = build_index_parts(&st);
    println!("index build: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let file = coqi::serialize(&parts, &st.schema, &Sha256::digest(&bytes).into());
    println!("{:<24} {:>12} {:>12}", "section", "raw", "zstd");
    let (mut r, mut z) = (0, 0);
    for s in coqi::section_sizes(&parts, &st.schema) {
        println!("{:<24} {:>12} {:>12}", s.name, mb(s.raw as u64), mb(s.zstd as u64));
        r += s.raw;
        z += s.zstd;
    }
    let whole = coqi::compress(&file).len();
    println!("{:<24} {:>12} {:>12}   (file {}, whole-file zstd {}; dataset {})", "total", mb(r as u64), mb(z as u64), mb(file.len() as u64), mb(whole as u64), mb(bytes.len() as u64));
    let t = Instant::now();
    let (back, dsha) = coqi::deserialize(&file, &st.schema).unwrap_or_else(|e| die(&format!("reload failed: {e}")));
    println!("reload (owned copy): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    if coqi::serialize(&back, &st.schema, &dsha) != file {
        die("round trip differs");
    }
    let (mut agree, mut total) = (0, 0);
    let samples: &[&str] = if st.schema.id == "mtg" { &SAMPLE_QUERIES } else { &[] };
    for q in samples {
        let g = coqc_mtg::Mtg.parse_gquery(q, &st, &[]).unwrap_or_else(|e| die(&format!("{q}: {e}")));
        total += 1;
        agree += (run_gquery(&st, &parts, &g).rows == run_gquery(&st, &back, &g).rows) as usize;
    }
    println!("round trip: byte-identical; {agree}/{total} sample queries agree");
    if agree != total {
        die("query mismatch after reload");
    }
    std::fs::write(&out, &file).unwrap_or_else(|e| die(&e.to_string()));
    println!("wrote {out}");
}

fn cmd_index_diff(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc index-diff <old> <new> [-o delta]");
    }
    let old = index_file(&args[0]);
    let new = index_file(&args[1]);
    let t = Instant::now();
    let (delta, info) = coqi::make_delta(&old, &new).unwrap_or_else(|e| die(&e.to_string()));
    println!("delta built in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    println!("{:<24} {:>9} {:>12} {:>12}", "section", "changed", "raw", "zstd");
    for tb in &info.tables {
        println!("{:<24} {:>9} {:>12} {:>12}", tb.name, tb.changed, mb(tb.raw as u64), mb(tb.zstd as u64));
    }
    println!("trigram postings: {} appended, {} new grams, {} replaced, {} removed", info.tri_appended, info.tri_new, info.tri_replaced, info.tri_removed);
    let again = coqi::apply_delta(&old, &delta).unwrap_or_else(|e| die(&format!("replay failed: {e}")));
    if again != new {
        die("replay differs from target");
    }
    println!("delta: raw {} -> {} (whole delta zstd); replay reproduces the new index byte-identically", mb(info.raw as u64), mb(delta.len() as u64));
    println!("for scale: new index file {} (zstd {}), old {}", mb(new.len() as u64), mb(coqi::compress(&new).len() as u64), mb(old.len() as u64));
    if let Some(out) = flag_value(args, "-o") {
        std::fs::write(&out, &delta).unwrap_or_else(|e| die(&e.to_string()));
        println!("wrote {out}");
    }
}

fn cmd_index_apply(args: &[String]) {
    if args.len() < 2 {
        die("usage: coqc index-apply <old.coqi> <delta.coqid> -o <new.coqi>");
    }
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o"));
    let (old, delta) = (rd(&args[0]), rd(&args[1]));
    let n = coqi::apply_delta(&old, &delta).unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(&out, &n).unwrap_or_else(|e| die(&e.to_string()));
    println!("applied: {out} ({}), hash verified", mb(n.len() as u64));
}

fn cmd_search(args: &[String]) {
    use coqc_format::gindex::GQuery;
    let path = args.first().unwrap_or_else(|| die("missing dataset"));
    let (_, st) = load_store(Path::new(path));
    // --overlay id=file.coqd (repeatable): attach overlays so their columns can be queried
    let ovs: Vec<(String, coqc_format::table::Store)> = args.windows(2).filter(|w| w[0] == "--overlay").map(|w| {
        let (id, f) = w[1].split_once('=').unwrap_or_else(|| die("--overlay id=file.coqd"));
        (id.to_string(), load_store(Path::new(f)).1)
    }).collect();
    let ov_refs: Vec<(String, &coqc_format::table::Store)> = ovs.iter().map(|(i, s)| (i.clone(), s)).collect();
    let mut query: GQuery = match flag_value(args, "--plan") {
        Some(j) => serde_json::from_str(&j).unwrap_or_else(|e| die(&format!("bad --plan: {e}"))),
        None => {
            let mut words = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--cards" => {}
                    "--limit" | "--offset" | "--plan" | "--overlay" => i += 1,
                    w => words.push(w.to_string()),
                }
                i += 1;
            }
            coqc::dialect_for(&st).parse_gquery(&words.join(" "), &st, &ov_refs).unwrap_or_else(|e| die(&e))
        }
    };
    if args.iter().any(|a| a == "--cards") {
        query.unique_parent = true;
    }
    query.limit = Some(flag_value(args, "--limit").map_or(20, |v| v.parse().unwrap_or_else(|_| die("bad --limit"))));
    query.offset = flag_value(args, "--offset").map_or(0, |v| v.parse().unwrap_or_else(|_| die("bad --offset")));
    let t = Instant::now();
    let parts = build_index_parts(&st);
    let built = t.elapsed();
    let t = Instant::now();
    let r = run_gquery_ov(&st, &parts, &query, ov_refs);
    let took = t.elapsed();
    println!("{} matches (index {} ms, query {:.2} ms)", r.total, built.as_millis(), took.as_secs_f64() * 1000.0);
    if st.schema.id != "mtg" {
        let d = coqc::dialect_for(&st);
        for row in r.rows {
            println!("  {:>6}  {}", row, d.record_json(&st, row));
        }
        return;
    }
    for row in r.rows {
        let p = coqc_mtg::tables::printing_at(&st, row as usize).unwrap_or_else(|e| die(&e.to_string()));
        let o = coqc_mtg::tables::oracle_at(&st, p.oracle_row as usize).unwrap_or_else(|e| die(&e.to_string()));
        println!(
            "  {:>6}  {} {:<5} {:<5} {}",
            row,
            p.released_at.map_or("-".into(), |d| coqc_mtg::json::released_to_string(d)),
            p.set_code.as_deref().unwrap_or("-"),
            p.collector_number.as_deref().unwrap_or("-"),
            o.name.as_deref().unwrap_or("?")
        );
    }
}

fn read_text(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| die(&format!("{p}: {e}")))
}

fn read_schema(p: &str) -> coqc_format::schema::Schema {
    serde_json::from_str(&read_text(p)).unwrap_or_else(|e| die(&format!("{p}: bad schema JSON: {e}")))
}

fn write_coqd(st: &coqc_format::table::Store, out: &str) {
    let (bytes, sections) = coqc_format::write_store(st).unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(out, &bytes).unwrap_or_else(|e| die(&format!("{out}: {e}")));
    let sha = sha_hex(&bytes);
    let fname = PathBuf::from(out).file_name().unwrap().to_string_lossy().to_string();
    std::fs::write(format!("{out}.sha256"), format!("{sha}  {fname}\n")).ok();
    for s in &sections {
        println!("    {:<15} raw {:>11}  stored {:>11}  blocks {}", s.name, mb(s.raw_bytes), mb(s.stored_bytes), s.blocks);
    }
    println!("epoch {}  file {}  sha256 {sha}", st.epoch, mb(bytes.len() as u64));
}

/// coqc generic-compile <schema.json> --table name=file.jsonl ... -o out.coqd [--prev p.coqd] [--rebase] [--source-date N]
fn cmd_generic_compile(args: &[String]) {
    let schema = read_schema(args.first().unwrap_or_else(|| die("missing schema.json")));
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqd>"));
    let inputs: Vec<(String, String)> = args.windows(2).filter(|w| w[0] == "--table").map(|w| {
        let (n, f) = w[1].split_once('=').unwrap_or_else(|| die("--table name=file.jsonl"));
        (n.to_string(), read_text(f))
    }).collect();
    let prev = flag_value(args, "--prev").map(|p| load_store(Path::new(&p)).1);
    let date = flag_value(args, "--source-date").map_or(0, |s| s.parse().unwrap_or_else(|_| die("bad --source-date")));
    let (st, stats) = coqc_generic::ingest::compile(&schema, &inputs, prev.as_ref(), args.iter().any(|a| a == "--rebase"), date).unwrap_or_else(|e| die(&e));
    for s in &stats {
        println!("{:<12} {} lines: {} live ({} new, {} changed, {} revived, {} tombstoned)", s.table, s.lines, s.live, s.new, s.changed, s.revived, s.tombstoned);
    }
    write_coqd(&st, &out);
}

/// coqc generic-overlay <overlay-schema.json> <base.coqd> <in.jsonl> -o out.coqd [--source-date N]
fn cmd_generic_overlay(args: &[String]) {
    if args.len() < 3 {
        die("usage: coqc generic-overlay <overlay-schema.json> <base.coqd> <in.jsonl> -o <out.coqd>");
    }
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <out.coqd>"));
    let date = flag_value(args, "--source-date").map_or(0, |s| s.parse().unwrap_or_else(|_| die("bad --source-date")));
    let (st, s) = coqc_generic::ingest::compile_overlay(&read_schema(&args[0]), &load_store(Path::new(&args[1])).1, &read_text(&args[2]), date).unwrap_or_else(|e| die(&e));
    println!("overlay: {} rows, {} lines, {} values, {} unknown keys skipped", s.rows, s.lines, s.values, s.unknown_keys);
    write_coqd(&st, &out);
}

/// coqc demo-books <dir> [--works N] [--day D]: the synthetic S5 dataset (schema, overlay schema, JSONL for that day).
fn cmd_demo_books(args: &[String]) {
    let dir = args.first().unwrap_or_else(|| die("missing output directory"));
    let n = flag_value(args, "--works").map_or(200, |s| s.parse().unwrap_or_else(|_| die("bad --works")));
    let day = flag_value(args, "--day").map_or(1, |s| s.parse().unwrap_or_else(|_| die("bad --day")));
    let d = coqc_generic::demo::books(n, day);
    std::fs::create_dir_all(dir).unwrap_or_else(|e| die(&e.to_string()));
    for (f, t) in [("schema.json", coqc_generic::demo::SCHEMA), ("overlay-schema.json", coqc_generic::demo::OVERLAY_SCHEMA), ("works.jsonl", &d.works), ("editions.jsonl", &d.editions), ("prices.jsonl", &d.prices)] {
        std::fs::write(format!("{dir}/{f}"), t).unwrap_or_else(|e| die(&e.to_string()));
    }
    println!("wrote books day {day} ({n}+ works) to {dir}");
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
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <prices.coqd>"));
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
    for (n, c) in coqc::prices::COLUMNS.iter().zip(r.per_column) {
        println!("  {n:<11} {c}");
    }
    println!("prices {out}: {} ({:.1}s)", mb(b.len() as u64), t0.elapsed().as_secs_f64());
}

fn cmd_prices_patch(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <p.coqp>"));
    if args.len() < 2 {
        die("usage: coqc prices-patch <old.coqd> <new.coqd> -o <p.coqp>");
    }
    let (a, b) = (rd(&args[0]), rd(&args[1]));
    let (p, _) = coqc_format::patch::make_patch(&a, &b).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, &p).unwrap_or_else(|e| die(&e.to_string()));
    println!("price delta {out}: {} vs full file {}", mb(p.len() as u64), mb(b.len() as u64));
}

fn cmd_prices_apply(args: &[String]) {
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o <new.coqd>"));
    if args.len() < 2 {
        die("usage: coqc prices-apply <old.coqd> <p.coqp> -o <new.coqd>");
    }
    let n = coqc_format::patch::apply_patch(&rd(&args[0]), &rd(&args[1])).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, &n).unwrap_or_else(|e| die(&e.to_string()));
    println!("applied: {out} ({}), hash verified", mb(n.len() as u64));
}

fn cmd_price(args: &[String]) {
    if args.len() < 3 {
        die("usage: coqc price <prices.coqd> <cards.coqd> <card name>");
    }
    let t = coqc::prices::PriceTable::decode(&rd(&args[0])).unwrap_or_else(|e| die(&e.to_string()));
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

fn cmd_zst(args: &[String]) {
    let inp = args.first().unwrap_or_else(|| die("usage: coqc zst <in> -o <out> [--level N]"));
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o"));
    let level: i32 = flag_value(args, "--level").map_or(19, |v| v.parse().unwrap_or_else(|_| die("bad --level")));
    let b = rd(inp);
    let z = zstd::encode_all(&b[..], level).unwrap_or_else(|e| die(&e.to_string()));
    std::fs::write(&out, &z).unwrap_or_else(|e| die(&e.to_string()));
    println!("{out}: {} -> {}", mb(b.len() as u64), mb(z.len() as u64));
}

fn cmd_unzst(args: &[String]) {
    let inp = args.first().unwrap_or_else(|| die("usage: coqc unzst <in.zst> -o <out>"));
    let out = flag_value(args, "-o").unwrap_or_else(|| die("missing -o"));
    let b = zstd::decode_all(&rd(inp)[..]).unwrap_or_else(|e| die(&format!("{inp}: {e}")));
    std::fs::write(&out, &b).unwrap_or_else(|e| die(&e.to_string()));
    println!("{out}: {}", mb(b.len() as u64));
}

fn measure(path: &str) -> coqc::manifest::Measured {
    let b = rd(path);
    let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
    coqc::manifest::Measured { name, size: b.len() as u64, sha256: coqc::manifest::sha_hex(&b) }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

/// `<prefix><stamp>.ext` -> stamp
fn stamp_of(path: &str, prefix: &str) -> String {
    let n = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path);
    let stem = n.strip_prefix(prefix).unwrap_or_else(|| die(&format!("{n}: expected a name starting with {prefix}")));
    stem.split('.').next().unwrap_or(stem).to_string()
}

/// `id=file[,patch=f][,of=id][,prefix=p]` (every occurrence of `flag`).
struct DsSpec {
    id: String,
    file: String,
    opts: Vec<(String, String)>,
}

fn ds_specs(args: &[String], flag: &str) -> Vec<DsSpec> {
    let mut v = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a != flag {
            continue;
        }
        let spec = args.get(i + 1).unwrap_or_else(|| die(&format!("{flag} needs id=file[,patch=f]")));
        let mut parts = spec.split(',');
        let (id, file) = parts
            .next()
            .and_then(|h| h.split_once('='))
            .unwrap_or_else(|| die(&format!("{flag} {spec}: expected id=file")));
        let opts = parts
            .map(|o| o.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or_else(|| die(&format!("{flag} {spec}: bad option {o}"))))
            .collect();
        v.push(DsSpec { id: id.to_string(), file: file.to_string(), opts });
    }
    v
}

fn cmd_manifest(args: &[String]) {
    use coqc::manifest::*;
    let req = |f: &str| flag_value(args, f).unwrap_or_else(|| die(&format!("missing {f}")));
    let out = req("-o");
    let base_url = flag_value(args, "--base-url").unwrap_or_else(|| {
        format!("https://github.com/{}/releases/download", flag_value(args, "--repo").unwrap_or_else(|| die("need --repo or --base-url")))
    });
    let (bases, indexes, overlays) = (ds_specs(args, "--base"), ds_specs(args, "--index"), ds_specs(args, "--overlay"));
    if bases.is_empty() {
        die("need at least one --base id=file.coqd");
    }
    let sole_base = (bases.len() == 1).then(|| bases[0].id.clone());
    let mut datasets: Vec<NewDataset> = Vec::new();
    let mut base_bytes: Vec<(String, Vec<u8>)> = Vec::new();
    let mut epoch: Option<u32> = None;
    for (role, specs) in [(Role::Base, &bases), (Role::Index, &indexes), (Role::Overlay, &overlays)] {
        for sp in specs.iter() {
            let opt = |k: &str| sp.opts.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            for (k, _) in &sp.opts {
                if !["patch", "of", "prefix"].contains(&k.as_str()) {
                    die(&format!("{}: unknown option {k}", sp.id));
                }
            }
            let of = if role == Role::Base { None } else { Some(opt("of").or_else(|| sole_base.clone()).unwrap_or_else(|| die(&format!("{}: needs of=<base id>", sp.id)))) };
            let prefix = opt("prefix").unwrap_or_else(|| format!("{}-", sp.id));
            let b = rd(&sp.file);
            let (format, min_reader, raw) = match role {
                Role::Index => {
                    let raw = zstd::decode_all(&b[..]).unwrap_or_else(|e| die(&format!("{}: not zstd: {e}", sp.file)));
                    if raw.get(..4) != Some(b"COQI") {
                        die(&format!("{}: not a compressed .coqi", sp.file));
                    }
                    let v = u16_at(&raw, 4);
                    // The index must belong to its base (its header carries the dataset's SHA-256).
                    let of_id = of.as_deref().unwrap_or_default();
                    let want = base_bytes.iter().find(|(i, _)| i == of_id).map(|(_, b)| sha_hex(b));
                    let have = raw.get(6..38).map(|s| s.iter().map(|b| format!("{b:02x}")).collect::<String>());
                    if want.is_none() || have != want {
                        die(&format!("{}: built from a different dataset than base {of_id}", sp.file));
                    }
                    (v, v, Some(("zstd".to_string(), raw.len() as u64, sha_hex(&raw))))
                }
                _ => {
                    if b.get(..4) != Some(b"COQD") {
                        die(&format!("{}: not a .coqd", sp.file));
                    }
                    if role == Role::Base {
                        let parsed = parse(&b).unwrap_or_else(|e| die(&format!("{}: {e}", sp.file)));
                        let e = parsed.meta().unwrap_or_else(|e| die(&e.to_string())).epoch;
                        if epoch.is_some_and(|x| x != e) {
                            die(&format!("{}: epoch {e} differs from the other base datasets", sp.file));
                        }
                        epoch = Some(e);
                        base_bytes.push((sp.id.clone(), b.clone()));
                    }
                    (u16_at(&b, 4), u16_at(&b, 6), None)
                }
            };
            datasets.push(NewDataset {
                id: sp.id.clone(),
                role,
                of,
                stamp: stamp_of(&sp.file, &prefix),
                prefix,
                format,
                min_reader,
                file: measure(&sp.file),
                raw,
                patch: opt("patch").map(|p| measure(&p)),
            });
        }
    }
    let inp = Inputs {
        base_url,
        run_tag: req("--run"),
        generated_at: req("--at"),
        keep_since: req("--keep-since"),
        force_rebase: args.iter().any(|a| a == "--rebase"),
        epoch: epoch.unwrap_or(0),
        datasets,
    };
    // A manifest of another version (or unreadable) is no usable history: the release starts a new chain.
    let prev: Option<Manifest> = flag_value(args, "--prev").and_then(|p| {
        let b = rd(&p);
        match serde_json::from_slice::<Manifest>(&b) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("note: {p} is not a v{MANIFEST_VERSION} manifest ({e}); starting a new chain");
                None
            }
        }
    });
    let m = build(&inp, prev.as_ref()).unwrap_or_else(|e| die(&e));
    std::fs::write(&out, serde_json::to_string_pretty(&m).unwrap() + "\n").unwrap_or_else(|e| die(&e.to_string()));
    let list: Vec<String> = m.datasets.iter().map(|d| format!("{} {} (chain {})", d.id, d.stamp, d.patches.len())).collect();
    println!("{out}: epoch {} rebase {} datasets {}", m.epoch, m.rebase, list.join(", "));
    // For scripts: names that belong in data-latest (full files) and in the run's release (patches).
    println!("latest: {}", current_files(&m).join(" "));
    println!("run: {}", run_files(&m).join(" "));
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
        "phash-eval" => cmd_phash_eval(rest),
        "generic-compile" => cmd_generic_compile(rest),
        "generic-overlay" => cmd_generic_overlay(rest),
        "demo-books" => cmd_demo_books(rest),
        "idmap" => cmd_idmap(rest),
        "prices" => cmd_prices(rest),
        "prices-patch" => cmd_prices_patch(rest),
        "prices-apply" => cmd_prices_apply(rest),
        "price" => cmd_price(rest),
        "search" => cmd_search(rest),
        "index-stats" => cmd_index_stats(rest),
        "index-build" => cmd_index_build(rest),
        "index-diff" => cmd_index_diff(rest),
        "index-apply" => cmd_index_apply(rest),
        "patch" => cmd_patch(rest),
        "apply" => cmd_apply(rest),
        "zst" => cmd_zst(rest),
        "unzst" => cmd_unzst(rest),
        "manifest" => cmd_manifest(rest),
        _ => {
            print!("{USAGE}");
            exit(1)
        }
    }
}
