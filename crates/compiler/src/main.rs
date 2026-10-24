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
        let card = coqc_format::json::card_json(m, &ds.oracle[po.oracle_row as usize], po);
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
        let (o, p) = coqc::diff_keys(&da, &db);
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
        _ => {
            print!("{USAGE}");
            exit(1)
        }
    }
}
