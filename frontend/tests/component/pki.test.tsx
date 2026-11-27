/** The PKI attention list names each machine, dates its expiry and keeps the uncapped total visible. */
import type * as Router from '@tanstack/react-router';
import userEvent from '@testing-library/user-event';
import { screen, waitFor, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { Pki } from '@/features/admin/Pki';
import type { PkiInfo } from '@/generated/api-types';
import { setLocale } from '@/i18n';

import { show } from './support';

const { read,filters } = vi.hoisted(() => ({ read: vi.fn<() => Promise<PkiInfo>>(),filters:vi.fn() }));
vi.mock('@/api/admin', () => ({ pkiPageQuery:(q='',cursor?:string)=>({queryKey:['admin','pki',q,cursor],queryFn:()=>{filters(q,cursor);return read();}}) }));
vi.mock('@tanstack/react-router', async (load) => ({
  ...await load<typeof Router>(),
  Link: ({ to, params, search, className, children }: {
    to: string; params: { id: string }; search: { tab: string }; className?: string; children: ReactNode;
  }) => <a href={`${to.replace('$id', params.id)}?tab=${search.tab}`} className={className}>{children}</a>,
}));

beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { await setLocale('en'); });

function info(total = 0, shown = 0): PkiInfo {
  return {
    ca: {
      subject: 'Orch authority', not_before: '2026-01-01T00:00:00.000Z',
      not_after: '2036-01-01T00:00:00.000Z', fingerprint_sha256: 'ab'.repeat(32),
    },
    services: [],
    machines_expiring_30d: total,
    machines_expiring: Array.from({ length: shown }, (_, index) => ({
      machine: { id: `machine-${index}`, name: `Build ${index + 1}`, online: false, placement: 'managed', platform_os: 'linux' },
      not_after: new Date(Date.UTC(2026, 9, 15, index)).toISOString(),
    })),
  };
}

describe('machine certificates in Management', () => {
  it('dates and links every returned machine in expiry order and says when more than fifty are due', async () => {
    const pki = info(53, 50);
    read.mockResolvedValue(pki);
    show(<Pki />);
    const table = await screen.findByRole('table', { name: 'Machines with certificates ending soon' });
    const links = within(table).getAllByRole('link');
    expect(links).toHaveLength(50);
    pki.machines_expiring.forEach((entry, index) => {
      expect(links[index]).toHaveTextContent(entry.machine.name);
      expect(links[index]).toHaveAttribute('href', `/resources/machines/${entry.machine.id}?tab=certificate`);
      // Follow this machine's link instead of rescanning all fifty rows for every date.
      const row = links[index]?.closest('tr');
      const date = row?.querySelector('time');
      expect(date).toHaveAttribute('datetime', entry.not_after);
      expect(date).not.toBeEmptyDOMElement();
    });
    expect(screen.getByText('53 machines hold a certificate that ends within 30 days')).toBeVisible();
    expect(screen.getByRole('searchbox')).toBeVisible();
  });

  it('shows the empty state without a table when no certificate is due', async () => {
    read.mockResolvedValue(info());
    show(<Pki />);
    expect(await screen.findByText('No machine holds a certificate that ends within 30 days.')).toBeVisible();
    expect(screen.queryByRole('table', { name: 'Machines with certificates ending soon' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Showing the first/)).not.toBeInTheDocument();
  });

  it('also names the dates, machine links and full total in Chinese', async () => {
    await setLocale('zh-CN');
    const pki = info(53, 50);
    read.mockResolvedValue(pki);
    show(<Pki />);
    const table = await screen.findByRole('table', { name: '证书即将到期的机器' });
    expect(within(table).getByRole('columnheader', { name: '机器' })).toBeVisible();
    expect(within(table).getByRole('columnheader', { name: '到期时间' })).toBeVisible();
    expect(within(table).getByRole('link', { name: 'Build 1' })).toHaveAttribute('href', '/resources/machines/machine-0?tab=certificate');
    expect(screen.getByRole('searchbox')).toBeVisible();
  });
});

it('finds a later due machine natively and follows its real expiry continuation', async()=>{
  const first={...info(53,1),machines_next_cursor:'older-due'};
  read.mockResolvedValue(first);show(<Pki/>);
  await screen.findByText('Build 1');
  read.mockResolvedValue({...info(53,1),machines_expiring:[{...first.machines_expiring[0]!,machine:{...first.machines_expiring[0]!.machine,name:'Later due'}}]});
  await userEvent.click(screen.getByRole('button',{name:'Show more'}));
  await waitFor(()=>expect(filters).toHaveBeenLastCalledWith('','older-due'));
  await userEvent.type(screen.getByRole('searchbox'),'Later');
  await waitFor(()=>expect(filters).toHaveBeenLastCalledWith('Later',undefined));
  expect(await screen.findByText('Later due')).toBeVisible();
  await userEvent.clear(screen.getByRole('searchbox'));
  await waitFor(()=>expect(filters).toHaveBeenLastCalledWith('',undefined));
});
