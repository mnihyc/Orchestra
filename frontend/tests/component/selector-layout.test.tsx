/** Form choices retain visible consequences; compact toolbar choices keep hover/focus help. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { NewAccess } from '@/features/resources/NewAccess';
import { choiceCopy } from '@/i18n/choices';
import { Choice } from '@/ui/Choice';
import { ChoiceField } from '@/ui/ChoiceField';
import { Checkbox } from '@/ui/Checkbox';
import { Popover } from '@/ui/Popover';
import { RadioGroup } from '@/ui/RadioGroup';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Switch } from '@/ui/Switch';
import { show } from './support';

vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));

type Kind = 'all' | 'machine' | 'template';
function Filter({ cards }: { cards: boolean }) {
  const [value, setValue] = useState<Kind>('all');
  const options = [
    { value: 'all' as const, ...choiceCopy(i18n.t, 'environmentFilter.all') },
    { value: 'machine' as const, ...choiceCopy(i18n.t, 'environmentFilter.machine') },
    { value: 'template' as const, ...choiceCopy(i18n.t, 'environmentFilter.template') },
  ];
  return cards ? <RadioGroup label="Kinds" value={value} onValueChange={setValue} options={options} />
    : <SegmentedControl label="Kinds" value={value} onValueChange={setValue} segments={options} />;
}
afterEach(() => setLocale('en'));

for (const language of ['en', 'zh-CN'] as const) {
  it.each([false, true])(`keeps descriptions on forms and short names on segments in ${language}, form=%s`, async (cards) => {
    await setLocale(language);
    const user = userEvent.setup();
    const { container } = show(<Filter cards={cards} />);
    const allWords = choiceCopy(i18n.t, 'environmentFilter.all');
    const machineWords = choiceCopy(i18n.t, 'environmentFilter.machine');
    const all = screen.getByRole('radio', { name: allWords.name });
    const machine = screen.getByRole('radio', { name: machineWords.name });
    expect(all).toBeChecked();
    if (cards) {
      expect(all).toHaveAccessibleDescription(allWords.description);
      expect(machine).toHaveAccessibleDescription(machineWords.description);
      expect(screen.getByText(allWords.description)).toBeVisible();
      expect(screen.getByText(machineWords.description)).toBeVisible();
    } else {
      expect(all).not.toHaveAttribute('aria-describedby');
      expect(machine).not.toHaveAttribute('aria-describedby');
      expect(within(all).queryByText(allWords.description)).not.toBeInTheDocument();
      expect(container.querySelector('[data-choice-description], [data-choice-help]')).toBeNull();
    }
    await user.click(machine);
    expect(machine).toBeChecked();
    if (cards) {
      expect(screen.getByText(machineWords.description)).toBeVisible();
      expect(screen.getByText(allWords.description)).toBeVisible();
    } else {
      expect(screen.queryByText(machineWords.description)).not.toBeInTheDocument();
      expect(screen.queryByText(allWords.description)).not.toBeInTheDocument();
    }
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
  });

  it.each([false, true])(`keeps approval consequences available through pointer and keyboard in ${language}, form=%s`, async (radio) => {
    await setLocale(language);
    const user = userEvent.setup();
    const ask = choiceCopy(i18n.t, 'approvalMode.ask');
    const automatic = choiceCopy(i18n.t, 'approvalMode.auto');
    function Modes() {
      const [value, setValue] = useState('ask');
      const options = [{ value: 'ask', ...ask }, { value: 'auto', ...automatic }];
      return <ChoiceField label="Approval mode" options={options}>{radio ? <RadioGroup label="Approval mode" value={value} onValueChange={setValue} options={options} />
        : <SegmentedControl label="Approval mode" value={value} onValueChange={setValue} segments={options} />}</ChoiceField>;
    }
    const { container } = show(<Modes />);
    const current = screen.getByRole('radio', { name: ask.name });
    expect(current).toHaveAttribute('data-state', 'checked');
    if (radio) {
      expect(current).toHaveAccessibleDescription(ask.description);
      expect(screen.getByText(ask.description)).toBeVisible();
      expect(screen.getByText(automatic.description)).toBeVisible();
    } else {
      expect(container.querySelector('[data-choice-description]')).toBeNull();
      expect(screen.queryByText(ask.description)).not.toBeInTheDocument();
      await user.hover(current);
      expect(await screen.findByRole('tooltip')).toHaveTextContent(ask.description);
    }
    expect(current).toBeChecked();
    expect(current).toHaveAttribute('data-state', 'checked');
    await user.unhover(current);
    // jsdom has zero-sized boxes: send the move outside Radix's pointer grace area too.
    await user.pointer({ target: document.body, coords: { clientX: 100, clientY: 100 } });
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    await user.click(screen.getByRole('radio', { name: automatic.name }));
    const next = screen.getByRole('radio', { name: automatic.name });
    expect(next).toBeChecked();
    expect(next).toHaveAttribute('data-state', 'checked');
    expect(current).toHaveAttribute('data-state', 'unchecked');
    act(() => { current.focus(); });
    await user.keyboard('{ArrowRight>}');
    await waitFor(() => expect(next).toHaveFocus());
    await user.keyboard('{/ArrowRight}');
    // Arrow selection fires the radio's native click, which dismisses its tooltip. Re-entry reads it.
    await user.tab(); await user.tab({ shift: true });
    expect(next).toHaveFocus();
    if (radio) {
      expect(next).toHaveAccessibleDescription(automatic.description);
      expect(screen.getByText(ask.description)).toBeVisible();
    } else {
      expect(await screen.findByRole('tooltip')).toHaveTextContent(automatic.description);
      await waitFor(() => expect(screen.queryByText(ask.description)).not.toBeInTheDocument());
      expect(container.querySelector('[data-choice-description]')).toBeNull();
    }
    await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: 'Approval mode' }) }));
    const help = screen.getByRole('dialog', { name: 'Approval mode' });
    expect(help).toHaveTextContent(ask.name); expect(help).toHaveTextContent(ask.description);
    expect(help).toHaveTextContent(automatic.name); expect(help).toHaveTextContent(automatic.description);
    expect(next).toBeChecked();
    expect(container.querySelector('[data-choice-selected-description], [data-choice-descriptions]')).toBeNull();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    if (radio) expect(screen.getByText(ask.description)).toBeVisible();
    else await waitFor(() => expect(screen.queryByText(ask.description)).not.toBeInTheDocument());
  });

  it(`keeps a resource audience compact with all consequences behind its one question in ${language}`, async () => {
    await setLocale(language);
    const user = userEvent.setup();
    const changed = vi.fn();
    function Audience() {
      const [value, setValue] = useState<'only_me' | 'home' | 'defaults'>('only_me');
      return <NewAccess home="Acme" value={value} onChange={(next) => { setValue(next); changed(next); }} />;
    }
    const { container } = show(<Audience />);
    const members = screen.getByRole('radio', { name: choiceCopy(i18n.t, 'resourceAccess.home', { home: 'Acme' }).name });
    expect(members).toHaveTextContent('Acme');
    expect(container.querySelector('[data-choice-selected-description], [data-choice-description]')).toBeNull();
    const own = i18n.t('resources.access.gives.only_me', { home: 'Acme' });
    expect(screen.queryByText(own)).not.toBeInTheDocument();
    const question = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('resources.access.label') }) });
    await user.click(question);
    const help = screen.getByRole('dialog', { name: i18n.t('resources.access.label') });
    for (const audience of ['only_me', 'home', 'defaults'] as const) expect(help).toHaveTextContent(i18n.t(`resources.access.gives.${audience}`, { home: 'Acme' }));
    expect(within(help).getAllByText(i18n.t('resources.access.admins', { home: 'Acme' }), { exact: true })).toHaveLength(1);
    expect(changed).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    await user.click(members);
    expect(members).toBeChecked();
    expect(changed).toHaveBeenLastCalledWith('home');
    await user.unhover(members);
    await user.hover(members);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('resources.access.gives.home', { home: 'Acme' }));
    expect(container.querySelector('[data-choice-selected-description]')).toBeNull();
    expect(container.querySelector('[data-choice-descriptions], [data-choice-help]')).toBeNull();
  });
}

it('omits descriptions and guide buttons from callback-built in-page choices', () => {
  const { container } = show(<Choice presentation="compact" name="All" description="Show environments of both creation kinds." help={{ label: 'Guide to environments', onOpen: () => undefined }}>
    {(content, control) => <button {...control} type="button">{content}</button>}
  </Choice>);
  const option = screen.getByRole('button', { name: 'All' });
  expect(option).not.toHaveAttribute('aria-describedby');
  expect(container.querySelector('[data-choice-description], [data-choice-help]')).toBeNull();
});

it('keeps picker explanations inside their option and removes them when the picker closes', async () => {
  const user = userEvent.setup();
  show(<Popover label="Models" trigger={<button>Choose model</button>}><Choice name="Model" description="The model used for the next turn.">{(content, control) => <button {...control}>{content}</button>}</Choice></Popover>);
  expect(screen.queryByText('The model used for the next turn.')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Choose model' }));
  const option = screen.getByRole('button', { name: 'Model' });
  expect(option).toHaveAccessibleDescription('The model used for the next turn.');
  expect(option).toContainElement(screen.getByText('The model used for the next turn.'));
  await user.keyboard('{Escape}');
  expect(screen.queryByText('The model used for the next turn.')).not.toBeInTheDocument();
});

it('keeps form switch and checkbox consequences visible and independently offers additional help', async () => {
  const user = userEvent.setup();
  const guide = vi.fn();
  const { container } = show(<><Switch name="Keep awake" description="Keep this environment available." help={{ label: 'Guide', onOpen: guide }} checked onCheckedChange={vi.fn()} />
    <Checkbox label="Full access" description="Reserve this computer for Orch." tooltip="Orch can control this machine." checked onCheckedChange={vi.fn()} /></>);
  expect(screen.getByText('Keep this environment available.')).toBeVisible();
  expect(screen.getByText('Reserve this computer for Orch.')).toBeVisible();
  expect(screen.getByRole('switch', { name: 'Keep awake' })).toHaveAccessibleDescription('Keep this environment available.');
  const full = screen.getByRole('checkbox', { name: 'Full access' });
  expect(full).toHaveAttribute('data-state', 'checked');
  expect(screen.getByRole('switch', { name: 'Keep awake' })).toHaveAttribute('data-state', 'checked');
  expect(screen.queryByText('Orch can control this machine.')).not.toBeInTheDocument();
  await user.hover(full);
  expect(await screen.findByRole('tooltip')).toHaveTextContent('Orch can control this machine.');
  expect(full).toHaveAccessibleDescription('Reserve this computer for Orch. Orch can control this machine.');
  expect(full).toHaveAttribute('data-state', 'checked');
  await user.unhover(full);
  await user.pointer({ target: document.body, coords: { clientX: 100, clientY: 100 } });
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  const awake = screen.getByRole('switch', { name: 'Keep awake' });
  act(() => { awake.focus(); });
  expect(awake).toHaveAccessibleDescription('Keep this environment available.');
  expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  expect(awake).toHaveAttribute('data-state', 'checked');
  expect(container.querySelector('[data-choice-selected-description], [data-choice-descriptions]')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Guide' }));
  expect(guide).toHaveBeenCalledOnce();
  expect(container.querySelector('[data-choice-help]')).not.toBeNull();
});

it('selects segments with arrows, skips disabled options, wraps and keeps a selected option selected', async () => {
  const user = userEvent.setup();
  const changed = vi.fn();
  function View() {
    const [value, setValue] = useState('all');
    return <SegmentedControl label="View" value={value} onValueChange={(next) => { setValue(next); changed(next); }} segments={[
      { value: 'all', name: 'All' }, { value: 'closed', name: 'Closed', disabled: true }, { value: 'mine', name: 'Mine' },
    ]} />;
  }
  show(<View />);
  await user.tab();
  expect(screen.getByRole('radio', { name: 'All' })).toHaveFocus();
  // Radix defers roving focus to a task and selects on focus while the arrow is held.
  // Wait for that native focus event before releasing the key, as the browser does.
  await user.keyboard('{ArrowRight>}');
  await waitFor(() => {
    expect(screen.getByRole('radio', { name: 'Mine' })).toHaveFocus();
    expect(screen.getByRole('radio', { name: 'Mine' })).toBeChecked();
  });
  await user.keyboard('{/ArrowRight}');
  expect(changed).toHaveBeenLastCalledWith('mine');
  await user.keyboard('{ArrowDown>}');
  await waitFor(() => expect(screen.getByRole('radio', { name: 'All' })).toBeChecked());
  await user.keyboard('{/ArrowDown}');
  await user.keyboard('{ArrowLeft>}');
  await waitFor(() => expect(screen.getByRole('radio', { name: 'Mine' })).toBeChecked());
  await user.keyboard('{/ArrowLeft}');
  const count = changed.mock.calls.length;
  await user.click(screen.getByRole('radio', { name: 'Mine' }));
  expect(screen.getByRole('radio', { name: 'Mine' })).toBeChecked();
  expect(changed).toHaveBeenCalledTimes(count);
  expect(screen.getByRole('radio', { name: 'Closed' })).toBeDisabled();
});
