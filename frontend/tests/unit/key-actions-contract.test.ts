// @vitest-environment node
/// <reference types="node" />
/** The keyboard actions of the browser, with their shipped keys, are the ones the server checks a person's keys against. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

import { KEY_ACTION_IDS, KEY_ACTIONS } from '@/shell/keyActions';

const contract = resolve(import.meta.dirname, '../../../crates/contracts/api/src/personal.rs');

describe('the keyboard actions', () => {
  it("are the server's, in the same order and with the same shipped keys", () => {
    const text = readFileSync(contract, 'utf8');
    const table = /pub const KEY_ACTIONS: &\[\(&str, &str\)\] = &\[([\s\S]*?)\n\];/.exec(text)?.[1] ?? '';
    // A Rust string writes a backslash twice.
    const server = [...table.matchAll(/\("([^"]+)", "((?:[^"\\]|\\.)*)"\)/g)].map((pair) => [pair[1] ?? '', (pair[2] ?? '').replace(/\\(.)/g, '$1')]);
    expect(server).toEqual(KEY_ACTION_IDS.map((id) => [id, KEY_ACTIONS[id].shipped]));
  });
});
