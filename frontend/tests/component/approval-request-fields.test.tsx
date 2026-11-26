/** A tool's request for approval reads as what it was called with, in the tool's own names, with the call whole one click away. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';

import { RequestBody } from '@/features/approvals/Harness';
import { i18n } from '@/i18n';
import { show } from './support';

it('reads a web fetch as its address and its question, not as a tree of keys, and keeps the call one click away', async () => {
  show(<RequestBody request={{ request_kind: 'other', options: [], detail: { summary: 'https://example.com',
    raw: { tool: 'WebFetch', input: { url: 'https://example.com', prompt: 'What is the page title? Reply with just the title.' } } } }} />);
  const fields = document.querySelector<HTMLElement>('[data-request-fields]');
  if (fields === null) throw new Error('The request has its fields');
  expect(within(fields).getByText('url')).toBeVisible();
  expect(within(fields).getByText('https://example.com')).toBeVisible();
  expect(within(fields).getByText('What is the page title? Reply with just the title.')).toBeVisible();
  // The summary said the address again: it is not drawn twice.
  expect(screen.getAllByText('https://example.com')).toHaveLength(1);
  // The call whole is folded under its summary until it is asked for.
  for (const tree of screen.queryAllByText(/2 keys/)) expect(tree).not.toBeVisible();
  await userEvent.click(screen.getByText(i18n.t('approvals.raw')));
  expect(document.querySelector('details')).toHaveAttribute('open');
});

it('keeps the tree for a request whose call says nothing in words', () => {
  show(<RequestBody request={{ request_kind: 'other', options: [], detail: { summary: 'Allow the future tool?', raw: { method: 'item/futureTool/requestApproval', kind: 'futurePermission' } } }} />);
  expect(screen.getByText('Allow the future tool?')).toBeVisible();
  expect(document.querySelector('[data-request-fields]')).toBeNull();
  expect(document.querySelector('details')).toBeNull();
});
