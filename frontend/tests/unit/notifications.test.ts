/** The rules under the ways a person is told and the approvals that wait for them: minutes a route takes, quiet hours as the minutes the server holds, a bot named from its link, what a request reaches said in words, requests sent together drawn as one, a web address a webhook posts to, and the key a browser subscribes with. */
import { describe, expect, it } from 'vitest';

import { groupBatches } from '@/features/approvals/ApprovalList';
import { reachOf } from '@/features/approvals/Access';
import { botOf } from '@/features/settings/personal/Connect';
import { minutesOfTime, timeOfMinutes } from '@/features/settings/personal/Notifications';
import { keyBytes, keyText } from '@/features/settings/personal/push';
import { minutesOf } from '@/features/settings/personal/Routes';
import { isWebAddress } from '@/features/settings/personal/Webhook';
import type { Approval } from '@/generated/api-types';

describe('the minutes a route takes', () => {
  it('are whole minutes from one to a day, and nothing else', () => {
    expect(minutesOf('15')).toBe(15);
    expect(minutesOf(' 30 ')).toBe(30);
    expect(minutesOf('1')).toBe(1);
    expect(minutesOf('1440')).toBe(1440);
    for (const wrong of ['', '0', '1441', '-5', '2.5', '1e2', 'ten', '15 min']) expect(minutesOf(wrong), wrong).toBeNull();
  });
});

describe('quiet hours', () => {
  it('are the minutes after midnight the server holds, and the time a field shows', () => {
    expect(minutesOfTime('22:00')).toBe(1320);
    expect(minutesOfTime('07:05')).toBe(425);
    expect(minutesOfTime('0:00')).toBe(0);
    expect(timeOfMinutes(1320)).toBe('22:00');
    expect(timeOfMinutes(425)).toBe('07:05');
    expect(timeOfMinutes(0)).toBe('00:00');
    for (const minutes of [0, 59, 60, 425, 1439]) expect(minutesOfTime(timeOfMinutes(minutes))).toBe(minutes);
  });

  it('are no time where the field holds none', () => {
    for (const wrong of ['', '24:00', '12:60', '12', 'noon']) expect(minutesOfTime(wrong), wrong).toBeNull();
  });
});

describe('a bot', () => {
  it('is named from the link that opens it, so that the person sees whom to press Start in', () => {
    expect(botOf('https://t.me/acme_orch_bot?start=abc123')).toBe('@acme_orch_bot');
    expect(botOf('https://telegram.me/Some_Bot?start=x')).toBe('@Some_Bot');
    expect(botOf('https://example.org/acme_orch_bot')).toBeNull();
    expect(botOf('not a link')).toBeNull();
  });
});

describe('what a request reaches', () => {
  it('is said once for each of the permissions that open a terminal or reach a machine, in the order they are asked', () => {
    expect(reachOf(['thread.view', 'terminal.manage', 'files.manage', 'terminal.manage'])).toEqual(['approvals.reach.terminal_manage', 'approvals.reach.files_manage']);
  });

  it('is nothing for permissions that only talk to the agent', () => {
    expect(reachOf(['thread.view', 'thread.interact', 'chat.view'])).toEqual([]);
    expect(reachOf([])).toEqual([]);
  });
});

describe('requests sent together', () => {
  /** An approval with only what the grouping looks at. */
  const approval = (id: string, kind: Approval['kind'], batch: string | null) => ({ id, kind, batch_id: batch }) as unknown as Approval;

  it('stand as one card, at the place of the first, and everything else stands alone', () => {
    const groups = groupBatches([approval('a', 'access', 'b1'), approval('c', 'capability', null), approval('d', 'access', 'b1'), approval('e', 'access', null), approval('f', 'access', 'b2')]);
    expect(groups.map((group) => group.map((each) => each.id))).toEqual([['a', 'd'], ['c'], ['e'], ['f']]);
  });

  it('are not joined to an agent’s request that happens to carry the same batch', () => {
    expect(groupBatches([approval('a', 'harness', 'b1'), approval('b', 'harness', 'b1')]).map((group) => group.length)).toEqual([1, 1]);
  });
});

describe('the address of a webhook of one’s own', () => {
  it('is a web address that a message can be posted to', () => {
    expect(isWebAddress('https://hooks.example.org/ops')).toBe(true);
    expect(isWebAddress(' http://127.0.0.1:8080/x ')).toBe(true);
    for (const wrong of ['', 'hooks.example.org', 'ftp://example.org', 'javascript:alert(1)', '//example.org']) expect(isWebAddress(wrong), wrong).toBe(false);
  });
});

describe('the key a browser subscribes with', () => {
  it('is read from base64url text into bytes and written back the same, as the server stores it', () => {
    // Sixty-five bytes: the length of an uncompressed public key, whose text has to be padded to be read.
    const bytes = Uint8Array.from({ length: 65 }, (_, at) => (at * 7 + 3) % 256);
    const text = keyText(bytes.buffer);
    expect(text).not.toMatch(/[+/=]/);
    expect([...keyBytes(text)]).toEqual([...bytes]);
    expect(keyText(null)).toBe('');
  });
});
