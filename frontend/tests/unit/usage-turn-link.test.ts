/** Ledger navigation keeps the actual numbered turn and rejects malformed or unsafe ordinals. */
import { expect, it } from 'vitest';
import { turnOrdinal } from '@/features/thread/turnLink';

it('opens positive numbered turns without deriving a number from an identifier', () => {
  expect(turnOrdinal('12')).toBe(12);
  expect(turnOrdinal(2)).toBe(2);
  for (const value of [0,-1,'0','-1','2.5','1e2','12tail','0199d1a8','9007199254740992',undefined]) expect(turnOrdinal(value)).toBeUndefined();
});
