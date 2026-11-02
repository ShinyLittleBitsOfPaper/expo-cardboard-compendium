import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adaptAppQuery } from '../appQuery';

const ALL = ' unique:prints include:extras order:released direction:desc';

test('appQuery: plain Scryfall-style terms pass through and the all-printings directives are added', () => {
  assert.deepEqual(adaptAppQuery('t:creature o:"first strike" -c:wu mv<=2'), { query: 't:creature o:"first strike" -c:wu mv<=2' + ALL, tags: [] });
  assert.equal(adaptAppQuery('').query, ALL.trim());
  assert.equal(adaptAppQuery('bolt', { allPrintings: false }).query, 'bolt');
  // the caller's own directives win
  assert.equal(adaptAppQuery('bolt unique:cards').query, 'bolt unique:cards');
});

test('appQuery: tag terms are extracted for the app to resolve', () => {
  assert.deepEqual(adaptAppQuery('tag:Staples -tag:"to trade" t:land', { allPrintings: false }), {
    query: 't:land',
    tags: [{ name: 'Staples', negate: false }, { name: 'to trade', negate: true }],
  });
});

test('appQuery: set#number becomes s: + cn: (zero padding stripped, negation grouped)', () => {
  assert.equal(adaptAppQuery('fra#0123 bolt', { allPrintings: false }).query, 's:fra cn:123 bolt');
  assert.equal(adaptAppQuery('-lea#161', { allPrintings: false }).query, '-(s:lea cn:161)');
  assert.equal(adaptAppQuery('sos#0221a', { allPrintings: false }).query, 's:sos cn:221a');
  // a quoted name containing # is left alone
  assert.equal(adaptAppQuery('"a#1 b"', { allPrintings: false }).query, '"a#1 b"');
});
