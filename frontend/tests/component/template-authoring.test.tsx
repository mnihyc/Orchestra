/** Worked examples fill the form; recorded host facts and one guide entry explain how to proceed. */
import { fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ComponentProps, useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { checkTemplate } from '@/api/resources';
import { TemplateGuideLink } from '@/features/help/TemplateGuideLink';
import { TemplateEditor } from '@/features/resources/templates/TemplateEditor';
import { TemplateExamples } from '@/features/resources/templates/TemplateExamples';
import { HostAdvice } from '@/features/resources/templates/TemplateHosts';
import { i18n, setLocale } from '@/i18n';
import { hostTemplateSchema, templateSchema } from '../doubles/template-specs';
import { show } from './support';

const { openGuide } = vi.hoisted(() => ({ openGuide: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openGuide }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), checkTemplate: vi.fn() }));
vi.mock('@/ui/CodeView', () => ({ CodeView: ({ text, label, onChange }: { text: string; label: string; onChange?: (text: string) => void }) => <textarea aria-label={label} value={text} onChange={(event) => onChange?.(event.target.value)} /> }));

const sources = [
  { driver: 'docker', name: 'Docker', schema: templateSchema('environment', {}) },
  { driver: 'host', name: 'Host', schema: hostTemplateSchema('environment') },
] as const;

function Authoring() {
  const [driver, setDriver] = useState('docker');
  const [text, setText] = useState(JSON.stringify({ image: 'initial:1' }));
  return <><TemplateGuideLink /><TemplateExamples sources={sources} onChoose={(nextDriver, spec) => { setDriver(nextDriver); setText(JSON.stringify(spec)); }} />
    <TemplateEditor compact key={driver} driver={driver} kind="environment" schema={driver === 'host' ? sources[1].schema : sources[0].schema} text={text} onChange={setText} /></>;
}

const offline: ComponentProps<typeof HostAdvice>['hosts'][number] = {
  id: 'host-1', name: 'docker-1', offline_since: '2026-10-15T09:00:00Z',
  machine: { id: 'machine-1', name: 'docker-machine', online: false, placement: 'managed', platform_os: 'linux' },
  drivers: [{ name: 'docker', title: 'Docker', creates: ['environment'], runtime_classes: [] }],
};

beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  vi.mocked(checkTemplate).mockResolvedValue({ valid: true, errors: [] });
});

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it('fills the selected Docker image or host program and keeps that same configuration in JSON', { timeout: 30_000 }, async () => {
  const user = userEvent.setup();
  show(<Authoring />);
  await user.click(screen.getByRole('button', { name: 'Start from an example' }));
  await user.click(screen.getByRole('menuitem', { name: 'Node' }));
  expect(screen.getByLabelText('Image reference')).toHaveValue('node:22-bookworm');
  expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  await user.click(screen.getByRole('link', { name: i18n.t('templates.editor.sections.uses') }));
  expect(within(screen.getByRole('group', { name: 'Limits' })).getByRole('spinbutton', { name: 'Processor time' })).toHaveValue(4000);
  await user.click(screen.getByRole('button', { name: 'Start from an example' }));
  await user.click(screen.getByRole('menuitem', { name: 'Host' }));
  expect(screen.getByLabelText('Run as user')).toHaveValue('agent');
  expect(screen.queryByLabelText('Image reference')).not.toBeInTheDocument();
  await user.click(screen.getByRole('link', { name: i18n.t('templates.editor.sections.commands') }));
  await user.click(screen.getByRole('button',{name:'Edit Commands after start'}));
  expect(screen.getByLabelText('Program and arguments · one per line')).toHaveValue('/usr/bin/printf\nHost tools are ready.');
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  await user.click(await screen.findByRole('switch', { name: 'JSON' }));
  const value = JSON.parse(screen.getByLabelText<HTMLTextAreaElement>('Configuration (JSON)').value) as Record<string, unknown>;
  expect(value).toEqual({ run_as: 'agent', after_up: [{ name: '10-ready', command: ['/usr/bin/printf', 'Host tools are ready.'] }] });
  await user.click(screen.getByRole('button', { name: 'Start from an example' }));
  await user.click(screen.getByRole('menuitem', { name: 'Rust' }));
  expect(screen.getByLabelText('Image reference')).toHaveValue('rust:1-bookworm');
});

it('has one named guide for the choices and opens it without changing the selected example', async () => {
  const user = userEvent.setup();
  const { container } = show(<Authoring />);
  expect(container.querySelectorAll('[data-help-article="write-an-environment-template"]')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Write an environment template' }));
  expect(openGuide).toHaveBeenCalledOnce();
  expect(openGuide).toHaveBeenLastCalledWith('write-an-environment-template');
  expect(screen.getByLabelText('Image reference')).toHaveValue('initial:1');
  expect(screen.queryByRole('button', { name: 'About Provider' })).not.toBeInTheDocument();
});

it.each([
  { locale: 'en' as const, title: 'Enrolled but offline', since: /docker-1 · offline since/ },
  { locale: 'zh-CN' as const, title: '已登记，但已离线', since: /docker-1 · 自/ },
])('names an offline enrolled host and its actual disconnection time in $locale', async ({ locale, title, since }) => {
  await setLocale(locale);
  const { container } = show(<HostAdvice hosts={[offline]} kind="environment" driver={null} hasSchema={false} />);
  expect(screen.getByText(title)).toBeVisible();
  expect(screen.getByText(since)).toBeVisible();
  expect(container.querySelector('time')).toHaveAttribute('datetime', offline.offline_since);
  expect(screen.queryByText('No host available')).not.toBeInTheDocument();
});

it('offers enrollment or access only when no host is visible', () => {
  show(<HostAdvice hosts={[]} kind="environment" driver={null} hasSchema={false} />);
  expect(screen.getByText('No host available')).toBeVisible();
  expect(screen.getByText('No host is enrolled or shared with you yet. Enroll one or ask its owner for access.')).toBeVisible();
});

it('says the offline time was not recorded rather than guessing it', () => {
  const { container } = show(<HostAdvice hosts={[{ ...offline, offline_since: null }]} kind="environment" driver="docker" hasSchema />);
  expect(screen.getByText('docker-1 · offline; the disconnection time was not recorded')).toBeVisible();
  expect(container.querySelector('time')).toBeNull();
  expect(screen.getByText(/You can write and save using/)).toBeVisible();
});

it('distinguishes an online host without a matching report from one that needs enrollment', () => {
  show(<HostAdvice hosts={[{ ...offline, machine: { ...offline.machine, online: true } }]} kind="environment" driver={null} hasSchema={false} />);
  expect(screen.getByText('No matching driver reported')).toBeVisible();
  expect(screen.queryByText(/Enroll one/)).not.toBeInTheDocument();
});

it('needs no warning when a visible online host reports the selected driver', () => {
  show(<HostAdvice hosts={[{ ...offline, machine: { ...offline.machine, online: true } }]} kind="environment" driver="docker" hasSchema />);
  expect(screen.queryByText('Enrolled but offline')).not.toBeInTheDocument();
  expect(screen.queryByText('No matching driver reported')).not.toBeInTheDocument();
});

it('shows and edits the Local embeddings Dockerfile with the ordinary provider template editor', async () => {
  const dockerfile = 'FROM llama-server\nRUN fetch embeddinggemma-2\n';
  const recipe = {build: {dockerfile, context: null, args: {}, target: null}, env: {LLAMA_ARG_ALIAS: 'embeddinggemma-2'}};
  const schema = templateSchema('provider', {properties: {build: {type: 'object', title: 'Build', properties: {dockerfile: {type: 'string', title: 'Dockerfile'}}}}});
  let changed = '';
  function LocalTemplate() {
    const [text, setText] = useState(JSON.stringify(recipe));
    return <TemplateEditor schema={schema} driver="docker" kind="provider" text={text} onChange={(next) => {changed = next; setText(next);}} />;
  }
  show(<LocalTemplate />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('switch', {name: 'JSON'}));
  const editor = screen.getByRole('textbox', {name: 'Configuration (JSON)'});
  expect(editor).toBeEnabled();
  expect((editor as HTMLTextAreaElement).value).toContain('embeddinggemma-2');
  const next = {...recipe, build: {...recipe.build, dockerfile: dockerfile + 'ENV LLAMA_ARG_THREADS=2\n'}};
  fireEvent.change(editor, {target: {value: JSON.stringify(next)}});
  expect(JSON.parse(changed)).toEqual(next);
});
