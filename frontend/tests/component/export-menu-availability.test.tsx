/** The same Export row explains unknown, failed and refused decisions in each menu. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { exportEntries } from '@/features/export/menu';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Menu } from '@/ui/Menu';
import { ContextMenu } from '@/ui/ContextMenu';
import { show } from './support';

const calls = { export: vi.fn<() => void>(), request: vi.fn<() => void>(), retry: vi.fn<() => void>() };
const appearance = useAppearance.getState();
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function Scene({ allowed, where, failed = false }: { allowed: boolean | null; where: 'row' | 'screen'; failed?: boolean }) {
  const entries = exportEntries({ allowed, where, owner: null, onExport: calls.export, onRequest: calls.request, t: i18n.t.bind(i18n),
    ...(failed ? { checkFailure: { reason: i18n.t('network.failed'), retry: calls.retry } } : {}) });
  return <ContextMenu label="Object menu" entries={entries}><div data-testid="object-row"><Menu label="Object menu" entries={entries} trigger={<button>Actions</button>} /></div></ContextMenu>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const where of ['row', 'screen'] as const) {
  it(`retains checking and refused Export in the ${where} menu in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    const view = show(<Scene allowed={null} where={where} />);
    await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
    const menu = screen.getByRole('menu');
    const exporting = within(menu).getByRole('menuitem', { name: i18n.t('export.menu.export') });
    expect(exporting).toHaveAttribute('aria-disabled', 'true');
    expect(menu).toHaveTextContent(i18n.t('export.menu.checking'));
    expect(within(menu).queryByRole('menuitem', { name: i18n.t('export.menu.request') })).not.toBeInTheDocument();
    await userEvent.click(exporting); expect(calls.export).not.toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');
    view.rerender(<Scene allowed={false} where={where} />);
    await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
    expect(screen.getByRole('menuitem', { name: i18n.t('export.menu.export') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('menu')).toHaveTextContent(i18n.t('export.menu.notAllowedNoOwner'));
    await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('export.menu.request') }));
    expect(calls.request).toHaveBeenCalledOnce(); expect(calls.export).not.toHaveBeenCalled();
  });

  it(`retries a failed ${where} read without offering an access request in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    show(<Scene allowed={null} where={where} failed />);
    await userEvent.pointer({ keys: '[MouseRight]', target: screen.getByTestId('object-row') });
    const menu = screen.getByRole('menu');
    expect(menu).toHaveTextContent(i18n.t('network.failed'));
    expect(menu).not.toHaveTextContent(i18n.t('export.menu.checking'));
    expect(within(menu).getByRole('menuitem', { name: i18n.t('export.menu.export') })).toHaveAttribute('aria-disabled', 'true');
    expect(within(menu).queryByRole('menuitem', { name: i18n.t('export.menu.request') })).not.toBeInTheDocument();
    await userEvent.click(within(menu).getByRole('menuitem', { name: i18n.t('common.tryAgain') }));
    expect(calls.retry).toHaveBeenCalledOnce(); expect(calls.request).not.toHaveBeenCalled(); expect(calls.export).not.toHaveBeenCalled();
  });
}

it('preserves enabled exports and direct-chat membership refusals', async () => {
  const view = show(<Scene allowed where="row" />);
  await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('export.menu.export') }));
  expect(calls.export).toHaveBeenCalledOnce(); view.unmount();
  const entries = exportEntries({ allowed: false, where: 'row', owner: null, reason: i18n.t('export.menu.onlyThePeople'),
    onExport: calls.export, onRequest: calls.request, t: i18n.t.bind(i18n) });
  show(<Menu label="Chat menu" entries={entries} trigger={<button>Actions</button>} />);
  await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
  expect(screen.getByRole('menu')).toHaveTextContent(i18n.t('export.menu.onlyThePeople'));
  expect(screen.queryByRole('menuitem', { name: i18n.t('export.menu.request') })).not.toBeInTheDocument();
  expect(calls.request).not.toHaveBeenCalled();
});

// Browser path: visible document row → Actions/right click → Export reason → Request access or failed-read Retry.
// 浏览器路径：可见文档行 → 操作菜单或右键 → 查看导出原因 → 申请权限，或重试失败的读取。
// The bilingual width/theme walk is in implicit-actions.spec.ts.
