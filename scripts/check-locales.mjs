#!/usr/bin/env node
// Key/placeholder parity check for the UI dictionaries.
//
// `en` lives in src/lib/i18n.svelte.ts next to `zh`; every other locale is a
// file under src/lib/locales/. A missing key silently falls back to Chinese,
// which is impossible to notice in a language you do not read, so CI must
// catch it. Placeholders are checked too: a dropped {0} would render an
// empty slot.
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const root = new URL('..', import.meta.url).pathname;
const i18n = readFileSync(join(root, 'src/lib/i18n.svelte.ts'), 'utf8');

const ENTRY = /'([^']+)':\s*'((?:[^'\\]|\\.)*)'/g;

function entriesOf(body) {
  const entries = new Map();
  for (const match of body.matchAll(ENTRY)) entries.set(match[1], match[2]);
  return entries;
}

function dictionary(source, name) {
  const start = source.indexOf(`const ${name}: Record<string, string> = {`);
  if (start < 0) throw new Error(`dictionary "${name}" not found`);
  const open = source.indexOf('{', start);
  const close = source.indexOf('\n};', open);
  return entriesOf(source.slice(open + 1, close));
}

function placeholders(value) {
  return [...value.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');
}

const en = dictionary(i18n, 'en');
const localesDir = join(root, 'src/lib/locales');
const files = readdirSync(localesDir).filter((f) => f.endsWith('.ts'));

let failures = 0;
for (const file of files) {
  const source = readFileSync(join(localesDir, file), 'utf8');
  const name = source.match(/export const (\w+): Record<string, string>/)?.[1];
  if (!name) {
    console.error(`${file}: no exported dictionary`);
    failures += 1;
    continue;
  }
  const entries = entriesOf(source.slice(source.indexOf('{')));
  const missing = [...en.keys()].filter((key) => !entries.has(key));
  const extra = [...entries.keys()].filter((key) => !en.has(key));
  const badPlaceholders = [...en.entries()].filter(
    ([key, value]) => entries.has(key) && placeholders(entries.get(key)) !== placeholders(value)
  );
  const empty = [...entries.keys()].filter((key) => entries.get(key).trim() === '');
  if (missing.length || extra.length || badPlaceholders.length || empty.length) {
    failures += 1;
    console.error(`${file}: ${missing.length} missing, ${extra.length} extra, ` +
      `${badPlaceholders.length} placeholder mismatches, ${empty.length} empty`);
    if (missing.length) console.error('  missing:', missing.slice(0, 8).join(', '));
    if (extra.length) console.error('  extra:', extra.slice(0, 8).join(', '));
    for (const [key, value] of badPlaceholders.slice(0, 5)) {
      console.error(`  ${key}: expected {${placeholders(value)}} got {${placeholders(entries.get(key))}}`);
    }
  } else {
    console.log(`${file}: ok (${entries.size} keys)`);
  }
}

if (files.length === 0) {
  console.error('no locale files found');
  failures += 1;
}
process.exit(failures === 0 ? 0 : 1);
