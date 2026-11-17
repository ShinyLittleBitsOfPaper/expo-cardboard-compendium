#!/usr/bin/env python3
"""CHANGELOG.md helper (used by cut-release.sh and package-bundle.sh).

  scripts/changelog.py message              commit/tag message body from the Unreleased entries
  scripts/changelog.py release X.Y.Z [DATE]  move Unreleased into a new "## X.Y.Z - DATE" section
  scripts/changelog.py check [BUNDLE_ID]     verify the entry format; BUNDLE_ID (e.g. 0.0.12-3, default: .bundle-seq)
                                             must have an entry in Unreleased

Entry format (one per change, newest first, continuation lines indented two spaces):
  - YYYY-MM-DD, X.Y.Z-N: **One-line summary.** Details ...
X.Y.Z-N is the bundle id (.bundle-seq): X.Y.Z is the version in Cargo.toml (the last release), N the package number.
The summary is the leading **bold** text; an entry without one falls back to its first sentence (cut at 140 chars).
"""
import datetime, os, re, sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
PATH = os.path.join(ROOT, "CHANGELOG.md")
ENTRY = re.compile(r"^- (\d{4}-\d{2}-\d{2}), (\d+\.\d+\.\d+-\d+): (.*)$")
MAX = 140


def read():
    return open(PATH, encoding="utf-8").read().split("\n")


def unreleased_span(lines):
    start = next((i for i, l in enumerate(lines) if l.strip() == "## Unreleased"), None)
    if start is None:
        sys.exit("CHANGELOG.md has no '## Unreleased' section")
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return start, end


def entries(lines, start, end):
    """[(date, bundle, first_line_rest, full_text)] in file order."""
    out, cur = [], None
    for l in lines[start + 1:end]:
        m = ENTRY.match(l)
        if m:
            cur = [m.group(1), m.group(2), [m.group(3)]]
            out.append(cur)
        elif l.startswith("- "):
            sys.exit(f"Unreleased entry does not match '- YYYY-MM-DD, X.Y.Z-N: text': {l[:80]}")
        elif cur is not None and l.strip():
            cur[2].append(l.strip())
    return [(d, b, t[0], " ".join(t)) for d, b, t in out]


def first_sentence(text):
    in_code = False
    for i, ch in enumerate(text):
        if ch == "`":
            in_code = not in_code
        elif not in_code and ch in ".!?" and (i + 1 == len(text) or text[i + 1] == " "):
            if text[max(0, i - 3):i + 1] in ("e.g.", "i.e."):
                continue
            return text[:i]
    return text


def summary(rest):
    m = re.match(r"\*\*(.+?)\*\*", rest)
    s = m.group(1) if m else first_sentence(rest)
    s = s.replace("**", "").replace("`", "").strip().rstrip(".:").strip()
    if len(s) > MAX:
        s = s[:MAX].rsplit(" ", 1)[0].rstrip(",;:") + "..."
    return s


def cargo_version():
    for l in open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8"):
        m = re.match(r'version = "(\d+\.\d+\.\d+)"', l)
        if m:
            return m.group(1)


def cmd_message():
    lines = read(); s, e = unreleased_span(lines)
    es = entries(lines, s, e)
    if not es:
        sys.exit("no entries under '## Unreleased' in CHANGELOG.md (every bundle needs one)")
    print("\n".join(f"- {d}, {b}: {summary(r)}" for d, b, r, _ in es))


def cmd_release(version, date):
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        sys.exit("version must be X.Y.Z")
    lines = read(); s, e = unreleased_span(lines)
    if not entries(lines, s, e):
        sys.exit("no entries under '## Unreleased' in CHANGELOG.md")
    if any(l.startswith(f"## {version}") for l in lines):
        sys.exit(f"CHANGELOG.md already has a section for {version}")
    body = lines[s + 1:e]
    while body and not body[0].strip(): body.pop(0)
    while body and not body[-1].strip(): body.pop()
    new = lines[:s + 1] + [""] + [f"## {version} - {date}", ""] + body + [""] + lines[e:]
    open(PATH, "w", encoding="utf-8").write("\n".join(new))
    print(f"CHANGELOG.md: Unreleased -> {version} - {date}")


def cmd_check(bundle):
    lines = read(); s, e = unreleased_span(lines)
    es = entries(lines, s, e)
    if bundle is None:
        p = os.path.join(ROOT, ".bundle-seq")
        bundle = open(p).read().strip() if os.path.exists(p) else None
    bad = 0
    if bundle and not any(b == bundle for _, b, _, _ in es):
        print(f"CHANGELOG.md: no entry for bundle {bundle} under Unreleased", file=sys.stderr); bad = 1
    cv = cargo_version()
    for d, b, r, _ in es:
        if cv and b.rsplit("-", 1)[0] != cv:
            print(f"CHANGELOG.md: entry {b} does not start with the Cargo.toml version {cv}", file=sys.stderr); bad = 1
        if not re.match(r"\*\*.+?\*\*", r):
            print(f"CHANGELOG.md: entry {b} has no **bold summary** (using its first sentence)", file=sys.stderr)
    sys.exit(bad)


def main(a):
    if a[:1] == ["message"]: cmd_message()
    elif a[:1] == ["release"] and len(a) in (2, 3): cmd_release(a[1], a[2] if len(a) == 3 else datetime.date.today().isoformat())
    elif a[:1] == ["check"] and len(a) <= 2: cmd_check(a[1] if len(a) == 2 else None)
    else: sys.exit(__doc__)


main(sys.argv[1:])
