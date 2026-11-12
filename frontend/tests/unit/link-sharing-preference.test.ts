/** Defaults and personal state remain facts, not guesses from malformed or hidden policy values. */
import { expect, it } from 'vitest';
import { linkPolicy } from '@/features/sharing/LinkSharing';
import { linkSharingPreference, timelineState } from '@/features/sharing/preference';
import type { ResolvedSetting } from '@/generated/api-types';

const row = (more: Partial<ResolvedSetting> = {}): ResolvedSetting => ({ key: 'sharing.on_link', value: true, own: null, locked_by: null,
  source: { scope: 'default', node: null }, parts: [], writable: false, ...more });

it('defaults on only for an absent personal choice and preserves opaque timeline entries', () => {
  expect(linkSharingPreference({})).toBe(true);
  const state = { timeline: { share_on_link: false, work_detail: 'steps', future: { retained: true } } };
  expect(linkSharingPreference(state)).toBe(false);
  expect({ ...timelineState(state), diff_layout: 'two' }).toEqual({ ...state.timeline, diff_layout: 'two' });
  expect(state.timeline).not.toHaveProperty('diff_layout');
});

it.each([{ timeline: [] }, { timeline: { share_on_link: 'false' } }])('keeps an unreadable preference unknown', (state) => {
  expect(linkSharingPreference(state)).toBeNull();
});

it('uses native policy values and the recorded lock instead of caller write authority', () => {
  expect(linkPolicy([row()])).toEqual({ on: true, lockedOff: false });
  expect(linkPolicy([row({ value: false })])).toEqual({ on: false, lockedOff: false });
  expect(linkPolicy([row({ value: false, locked_by: { scope: 'system', node: null } })])).toEqual({ on: false, lockedOff: true });
  expect(linkPolicy([row({ value: 'false' })]).on).toBeNull();
  expect(linkPolicy([]).on).toBeNull();
});
