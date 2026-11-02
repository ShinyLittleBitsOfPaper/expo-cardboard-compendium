// Adapter from slbop's own search grammar (src/utils/searchParser.ts) to the engine's Scryfall syntax. Pure, no I/O.
// Everything slbop accepts except `tag:` and `set#number` already is Scryfall syntax; those two are rewritten/extracted here.

export interface AppQuery {
  /** Engine query. */
  query: string;
  /** `tag:name` terms: the engine knows nothing about user tags, so the app resolves them (registerSet/registerRows + `scope`, or by filtering). */
  tags: { name: string; negate: boolean }[];
}

export interface AppQueryOptions {
  /** List every printing, tokens and other extras included, newest first (what slbop's card list does). Default true. */
  allPrintings?: boolean;
}

/** Split on whitespace outside double quotes. */
function tokens(raw: string): string[] {
  return raw.match(/(?:[^\s"]+|"[^"]*"?)+/g) ?? [];
}

export function adaptAppQuery(raw: string, o: AppQueryOptions = {}): AppQuery {
  const tags: AppQuery['tags'] = [];
  const out: string[] = [];
  for (const tok of tokens(raw)) {
    const neg = tok.startsWith('-') && tok.length > 1;
    const body = neg ? tok.slice(1) : tok;
    const tag = /^tag:(.+)$/i.exec(body);
    if (tag) {
      tags.push({ name: tag[1].replace(/^"|"$/g, ''), negate: neg });
      continue;
    }
    const sn = /^([a-z0-9]{2,6})#(\d\w*)$/i.exec(body);
    if (sn) {
      const num = sn[2].replace(/^0+(?=.)/, '');
      out.push(neg ? `-(s:${sn[1]} cn:${num})` : `s:${sn[1]} cn:${num}`);
      continue;
    }
    out.push(tok);
  }
  let query = out.join(' ');
  if (o.allPrintings !== false && !/(^|\s)(unique|order|direction|include):/i.test(query)) {
    query = `${query} unique:prints include:extras order:released direction:desc`.trim();
  }
  return { query, tags };
}
