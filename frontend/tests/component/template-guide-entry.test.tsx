/** The actual template Settings panel keeps its one localized shipped-guide entrance. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TemplateScreen } from '@/features/resources/templates/Template';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

const openGuide = vi.hoisted(() => vi.fn());
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(),
  templateQuery: () => ({ queryKey: ['guide-template'], queryFn: () => Promise.resolve(detailTemplate), staleTime: Infinity }),
  driversQuery: { queryKey: ['guide-template-drivers'], queryFn: () => Promise.resolve([{ name: detailTemplate.driver, descriptor: { spec_schemas: { environment: { type: 'object', properties: { image: { type: 'string', title: 'Image reference' } } } } } }]), staleTime: Infinity },
  checkTemplate: () => Promise.resolve({ valid: true, errors: [] }),
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/resources/templates/acts', () => ({ useTemplateActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/ResourceCreation', () => ({ ResourceCreateButton: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openGuide }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>, PageHeader: ({ title }: { title: string }) => <h1>{title}</h1> }));
const priorAppearance = useAppearance.getState();
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); useAppearance.setState(priorAppearance); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens the first-template guide once without leaving the draft (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const user = userEvent.setup();
    const view = show(<TemplateScreen templateId={detailTemplate.id} tab="spec" onTab={vi.fn()} />);
    const guide = await screen.findByRole('button', { name: i18n.t('templateHelp.title') });
    expect(view.container.querySelectorAll('[data-help-article="write-an-environment-template"]')).toHaveLength(1);
    const image = await screen.findByRole('textbox', { name: 'Image reference' });
    expect(image).toHaveValue('research:stable');
    await user.click(guide);
    expect(openGuide).toHaveBeenCalledExactlyOnceWith('write-an-environment-template');
    expect(image).toHaveValue('research:stable');
  });
}
