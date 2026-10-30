/** The code a person scans to set up their authenticator: the smallest symbol that holds the link, none for a link too long to scan, and a format word a scanner can read back. */
import { describe, expect, it } from 'vitest';

import { qrModules } from '@/lib/qr';

/** The format word as the first copy reads it: five bits of what the symbol says, ten of error correction. */
function formatWord(modules: readonly (readonly boolean[])[]): number {
  let word = 0;
  const read = (row: number, column: number, bit: number) => {
    if (modules[row]?.[column] === true) word |= 1 << bit;
  };
  for (let bit = 0; bit <= 5; bit += 1) read(bit, 8, bit);
  read(7, 8, 6);
  read(8, 8, 7);
  read(8, 7, 8);
  for (let bit = 9; bit < 15; bit += 1) read(8, 14 - bit, bit);
  return word;
}

/** Whether a word of fifteen bits is one of the thirty-two a symbol may carry, once the fixed pattern is taken off. */
function isFormatCodeword(word: number): boolean {
  const data = (word ^ 0x5412) >>> 10;
  let code = data << 10;
  for (let at = 14; at >= 10; at -= 1) if ((code >>> at) & 1) code ^= 0x537 << (at - 10);
  return ((data << 10) | code) === (word ^ 0x5412);
}

describe('the code to scan for an authenticator', () => {
  it('is the smallest symbol that holds the link', () => {
    expect(qrModules('hi')?.length).toBe(21);
    // Fourteen bytes is the most that version 1 holds at this level of correction; one more needs version 2.
    expect(qrModules('x'.repeat(14))?.length).toBe(21);
    expect(qrModules('x'.repeat(15))?.length).toBe(25);
    // A usual link of an authenticator, with a name of the system in it.
    const link = 'otpauth://totp/Orch:ada?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Orch';
    expect(qrModules(link)?.length).toBe(37);
  });

  it('is not drawn for a text longer than the largest symbol holds', () => {
    expect(qrModules('x'.repeat(213))?.length).toBe(57);
    expect(qrModules('x'.repeat(214))).toBeNull();
  });

  it('counts a text by its bytes, not by its letters', () => {
    // Seven letters of two bytes each take what fourteen plain ones do.
    expect(qrModules('é'.repeat(7))?.length).toBe(21);
    expect(qrModules('é'.repeat(8))?.length).toBe(25);
  });

  it('carries a format word that a scanner can read and check', () => {
    for (const text of ['hi', 'https://orch.example/invite/abcdefghijklmnopqrstuvwxyz', '王小明']) {
      const modules = qrModules(text);
      expect(modules).not.toBeNull();
      expect(isFormatCodeword(formatWord(modules ?? []))).toBe(true);
    }
  });

  it('has the three finder squares and the alternating timing line', () => {
    const modules = qrModules('hi') ?? [];
    const size = modules.length;
    for (const [row, column] of [
      [0, 0],
      [0, size - 7],
      [size - 7, 0],
    ] as const) {
      // The outer ring of a finder is dark all round and its centre is dark.
      expect(modules[row]?.slice(column, column + 7).every(Boolean)).toBe(true);
      expect(modules[row + 3]?.[column + 3]).toBe(true);
      expect(modules[row + 1]?.[column + 1]).toBe(false);
    }
    for (let at = 8; at < size - 8; at += 1) expect(modules[6]?.[at]).toBe(at % 2 === 0);
  });

  it('is the same symbol every time for the same text', () => {
    expect(qrModules('same')).toEqual(qrModules('same'));
  });
});
