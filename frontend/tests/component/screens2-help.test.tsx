/** Setting, choice and named guide entrances share a destination inside panels and pickers too. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { GuideLink } from '@/features/help/GuideLink';
import { GuideSettings } from '@/features/help/GuideSettings';
import { Hint } from '@/features/help/Hint';
import { SettingHelpGroup } from '@/features/help/SettingHelpGroup';
import { SettingHelpLink } from '@/features/help/SettingHint';
import { SettingGuideLink } from '@/features/help/SettingGuideLink';
import { settingHelpEntries } from '@/features/help/settings';
import { showArticle, useHelpState } from '@/features/help/state';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { Checkbox } from '@/ui/Checkbox';
import { Menu } from '@/ui/Menu';
import { Select } from '@/ui/Select';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => 'wide' }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn() }));
vi.mock('@/help', async () => {
  // This suite exercises entrance ownership. The separate content suite checks the real shipped files/assets.
  const { makeGuide } = await import('@/help/guide');
  const guide = makeGuide({
    'resources.md': '# Resources\n\n## Providers and models',
    'how-approvals-work.md': '# How approvals work',
    'let-agents-open-ports.md': '# Let agents open ports',
  });
  return { useGuide: () => guide };
});

const target = { kind: 'node', id: 'actual-thread' } as const;
function title(key: string): string {
  const def = settingHelpEntries(i18n).find((entry) => entry.key === key);
  if (def === undefined) throw new Error(`No shipped setting: ${key}`);
  return def.title;
}
function question(term: string): string { return i18n.t('help.hint.label', { term }); }

beforeEach(() => { calls.open.mockClear(); useHelpState.setState({ settingContext: null, guideContext: null }); });
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('opens the authored cleanup heading and keeps the actual host context in %s', async (locale) => {
  await setLocale(locale);
  const host = {kind:'host',id:'actual-host'} as const;
  show(<SettingGuideLink def={{key:'cleanup.unused_keep_days'}} target={host} />);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('storageCleanup.guide')}));
  expect(calls.open).toHaveBeenLastCalledWith('storage-cleanup','unused-caches');
  expect(useHelpState.getState().guideContext).toEqual({article:'storage-cleanup',target:host});
});

it.each(['en', 'zh-CN'] as const)('keeps the approval panel’s named guide and removes its repeated setting questions in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<><GuideLink guide="how-approvals-work" target={target} />
    <SettingHelpLink settingKey="approval.mode" target={target} />
    <SettingHelpLink settingKey="approval.allowed_modes" target={target} /></>);
  await waitFor(() => expect(view.container.querySelectorAll('[data-help-article="how-approvals-work"]')).toHaveLength(1));
  expect(screen.getByRole('button', { name: i18n.t('helpRelease.guides.approvals') })).toBeVisible();
  expect(screen.queryByRole('button', { name: question(title('approval.mode')) })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: question(title('approval.allowed_modes')) })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpRelease.guides.approvals') }));
  expect(useHelpState.getState().guideContext).toEqual({ article: 'how-approvals-work', target });
});

it.each(['en', 'zh-CN'] as const)('keeps the approval settings’ full entries reachable from their one guide in %s', async (locale) => {
  await setLocale(locale);
  const open = vi.fn();
  show(<GuideSettings article="how-approvals-work" onOpen={open} />);
  for (const key of ['approval.mode', 'approval.allowed_modes']) {
    await userEvent.click(screen.getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title(key) }) }));
    expect(open).toHaveBeenLastCalledWith(key);
  }
});

it.each(['en', 'zh-CN'] as const)('prefers the named panel guide even when an outbound-network hint comes first in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<><SettingHelpLink settingKey="network.allowed_hosts" />
    <GuideLink guide="let-agents-open-ports" target={{ kind: 'environment', id: 'actual-environment' }} /></>);
  await waitFor(() => expect(view.container.querySelectorAll('[data-help-article="let-agents-open-ports"]')).toHaveLength(1));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpRelease.guides.ports') }));
  expect(useHelpState.getState().guideContext).toEqual({ article: 'let-agents-open-ports', target: { kind: 'environment', id: 'actual-environment' } });
});

it('drops the originating place when another article is opened', () => {
  useHelpState.setState({ guideContext: { article: 'how-approvals-work', target } });
  showArticle('finding-things');
  expect(useHelpState.getState().guideContext).toBeNull();
});

it('promotes the remaining setting question when the named guide leaves, retaining the entry and its actual place', async () => {
  const first = show(<GuideLink guide="how-approvals-work" />);
  const remaining = show(<><SettingHelpLink settingKey="approval.mode" target={target} /><SettingHelpLink settingKey="approval.allowed_modes" target={target} /></>);
  await waitFor(() => expect(remaining.container.querySelectorAll('[data-help-article]')).toHaveLength(0));
  first.unmount();
  await userEvent.click(await screen.findByRole('button', { name: question(title('approval.mode')) }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title('approval.mode') }) }));
  expect(calls.open).toHaveBeenLastCalledWith('setting:approval.mode');
  expect(useHelpState.getState().settingContext).toEqual({ key: 'approval.mode', target });
  expect(screen.queryByRole('dialog', { name: title('approval.mode') })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: question(title('approval.mode')) }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpGroups.readGuide', { guide: i18n.t('helpRelease.guides.approvals') }) }));
  expect(calls.open).toHaveBeenLastCalledWith('how-approvals-work');
});

it.each(['en', 'zh-CN'] as const)('offers only Model help when no effort control is present in %s', async (locale) => {
  await setLocale(locale);
  const term = i18n.t('helpGroups.model');
  show(<SettingHelpGroup settingKeys={['agent.model']} target={target} term={term} article="resources" heading="providers-and-models" />);
  await userEvent.click(screen.getByRole('button', { name: question(term) }));
  const card = screen.getByRole('dialog', { name: term });
  const model = within(card).getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title('agent.model') }) });
  expect(within(card).queryByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title('agent.effort') }) })).not.toBeInTheDocument();
  await userEvent.click(model);
  expect(calls.open).toHaveBeenLastCalledWith('setting:agent.model');
  expect(useHelpState.getState().settingContext).toEqual({ key: 'agent.model', target });
  expect(card).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps one question inside a picker and every visible setting’s entry in %s', async (locale) => {
  await setLocale(locale);
  const term = i18n.t('helpGroups.modelAndEffort');
  const view = show(<><ArticleGuideLink article="resources" heading="providers-and-models" title="Underlying guide" />
    <section role="dialog" aria-label="Model picker example">
      <SettingHelpGroup settingKeys={['agent.model', 'agent.effort', 'agent.params', 'agent.params']} target={target} term={term} article="resources" heading="providers-and-models" />
      <Hint id="provider-kind" />
    </section></>);
  const picker = screen.getByRole('dialog', { name: 'Model picker example' });
  await waitFor(() => expect(picker.querySelectorAll('[data-help-article="resources"]')).toHaveLength(1));
  expect(view.container.querySelectorAll('[data-help-article="resources"]')).toHaveLength(2);
  await userEvent.click(within(picker).getByRole('button', { name: question(term) }));
  const card = screen.getByRole('dialog', { name: term });
  for (const key of ['agent.model', 'agent.effort', 'agent.params']) {
    expect(within(card).getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title(key) }) })).toBeVisible();
  }
  await userEvent.click(within(card).getByRole('button', { name: i18n.t('helpGroups.openSetting', { setting: title('agent.effort') }) }));
  expect(calls.open).toHaveBeenLastCalledWith('setting:agent.effort');
  expect(useHelpState.getState().settingContext).toEqual({ key: 'agent.effort', target });
});

it.each(['en', 'zh-CN'] as const)('keeps the panel guide once while its compact choices have no repeated questions in %s', async (locale) => {
  await setLocale(locale);
  const changed = vi.fn();
  const guide = { article: 'let-agents-open-ports', heading: null };
  const name = i18n.t('helpRelease.guides.ports');
  const view = show(<><GuideLink guide="let-agents-open-ports" />{(['place', 'link'] as const).map((kind) => {
    const copy = choiceCopy(i18n.t, `portAudience.${kind}`);
    return <Checkbox key={kind} label={copy.name} description={copy.description} checked={false} onCheckedChange={changed}
      help={{ label: name, guide, onOpen: () => { calls.open(guide.article); } }} />;
  })}</>);
  await waitFor(() => expect(screen.getAllByRole('button', { name })).toHaveLength(1));
  expect(screen.getAllByRole('checkbox')).toHaveLength(2);
  expect(view.container.querySelectorAll('[data-help-article="let-agents-open-ports"]')).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name }));
  expect(calls.open).toHaveBeenLastCalledWith('let-agents-open-ports');
  expect(changed).not.toHaveBeenCalled();
  for (const control of screen.getAllByRole('checkbox')) expect(control).not.toBeChecked();
});

it('keeps help at its panel entrance and preserves the menu’s checkable entries', async () => {
  const changed = vi.fn();
  const help = { label: 'Ports guide', guide: { article: 'let-agents-open-ports', heading: null }, onOpen: () => { calls.open('let-agents-open-ports'); } };
  show(<><GuideLink guide="let-agents-open-ports" /><Menu label="Audiences" trigger={<button>Choose audiences</button>} entries={(['place', 'link'] as const).map((kind) => ({
    type: 'check' as const, id: kind, ...choiceCopy(i18n.t, `portAudience.${kind}`), checked: false, onCheckedChange: changed, help,
  }))} /></>);
  await userEvent.click(screen.getByRole('button', { name: 'Choose audiences' }));
  expect(screen.queryByRole('menuitem', { name: 'Ports guide' })).not.toBeInTheDocument();
  expect(screen.getAllByRole('menuitemcheckbox')).toHaveLength(2);
  await userEvent.click(screen.getByRole('menuitemcheckbox', { name: i18n.t('choiceCatalog.portAudience.place.name') }));
  expect(changed).toHaveBeenCalledWith(true);
  await userEvent.keyboard('{Escape}');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpRelease.guides.ports') }));
  expect(calls.open).toHaveBeenLastCalledWith('let-agents-open-ports');
  expect(useHelpState.getState().guideContext).toBeNull();
});

it('retains a Select guide’s destination when moving it outside the listbox, including a disabled choice', async () => {
  const changed = vi.fn();
  const help = { label: 'Ports guide', guide: { article: 'let-agents-open-ports', heading: null }, onOpen: () => { calls.open('let-agents-open-ports'); } };
  show(<Select label="Audience" value="place" onValueChange={changed} options={[
    { value: 'place', ...choiceCopy(i18n.t, 'portAudience.place'), help },
    { value: 'link', ...choiceCopy(i18n.t, 'portAudience.link'), help, disabled: true },
  ]} />);
  await userEvent.click(screen.getByRole('combobox', { name: 'Audience' }));
  const popup = await screen.findByRole('dialog', { name: 'Audience' });
  await waitFor(() => expect(within(popup).getAllByRole('button', { name: 'Ports guide' })).toHaveLength(1));
  const entry = within(popup).getByRole('button', { name: 'Ports guide' });
  expect(entry).toHaveAttribute('data-help-article', 'let-agents-open-ports');
  expect(within(popup).getByRole('listbox')).not.toContainElement(entry);
  await userEvent.click(entry);
  expect(calls.open).toHaveBeenLastCalledWith('let-agents-open-ports');
  expect(changed).not.toHaveBeenCalled();
});
