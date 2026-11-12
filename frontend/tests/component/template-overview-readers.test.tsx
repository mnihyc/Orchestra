/** The actual template overview reads host reports only when it names a runtime. */
import { cleanup, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { TemplateScreen } from '@/features/resources/templates/Template';
import type { RuntimeClass, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailHost, detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ hosts: vi.fn(), drivers: vi.fn() }));
let template: Template = detailTemplate;
const runtime: RuntimeClass = {
  name: 'reported-runtime', title: 'Reported runtime', boundary: 'user_space_kernel', capabilities: {}, unavailable: {},
  live_changes: { gateway_given: [], resources: false, mounts: false, capabilities: [] }, network_egress: ['open'],
  recreate_preserves_files: false, keeps_software_on_stop: false, memory_overhead_mb: 64,
  limits: [{ key: 'sample_limit', touches: [], todo: null, why: 'A preserved runtime limitation' }],
};

vi.mock('@/api/resources', async (load) => ({ ...await load<object>(),
  templateQuery: () => ({ queryKey: ['overview-template'], queryFn: () => Promise.resolve(template), staleTime: Infinity }),
  driversQuery: { queryKey: ['overview-drivers'], queryFn: () => { reads.drivers(); return Promise.resolve([{ name: detailTemplate.driver, title: 'Example driver' }]); }, staleTime: Infinity },
  hostsQuery: () => ({ queryKey: ['overview-hosts'], queryFn: () => {
    reads.hosts();
    return Promise.resolve([{ ...detailHost, drivers: [{ ...detailHost.drivers[0], name: detailTemplate.driver, runtime_classes: [runtime.name], runtimes: [runtime] }] }]);
  }, staleTime: Infinity }),
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/resources/templates/acts', () => ({ useTemplateActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(),
  Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title }: { title: string }) => <h1>{title}</h1>,
}));

beforeEach(() => { vi.clearAllMocks(); template = { ...detailTemplate, spec: { image: 'research:stable' } }; });
afterEach(async () => { cleanup(); await setLocale('en'); });

it('keeps the visible driver and image without reading hosts when no runtime is declared', async () => {
  show(<TemplateScreen templateId={template.id} tab="overview" onTab={vi.fn()} />);
  await screen.findByText('Example driver');
  expect(screen.getByText('research:stable')).toBeVisible();
  expect(reads.drivers).toHaveBeenCalledOnce();
  expect(reads.hosts).not.toHaveBeenCalled();
});

it('still reads the declared runtime and renders its reported boundary and limits', async () => {
  template = { ...template, spec: { image: 'research:stable', runtime: runtime.name } };
  show(<TemplateScreen templateId={template.id} tab="overview" onTab={vi.fn()} />);
  await screen.findByText(i18n.t('isolation.hint', { runtime: runtime.title, boundary: i18n.t(`isolation.boundary.${runtime.boundary}`) }));
  expect(screen.getByText('A preserved runtime limitation')).toBeVisible();
  expect(reads.drivers).toHaveBeenCalledOnce();
  expect(reads.hosts).toHaveBeenCalledOnce();
});
