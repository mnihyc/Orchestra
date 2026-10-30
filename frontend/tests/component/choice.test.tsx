/** Familiar choice names stay separate from their explanations and guides, for the pointer and keyboard. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { Checkbox, CheckboxList } from '@/ui/Checkbox';
import { Choice } from '@/ui/Choice';
import { ContextMenu } from '@/ui/ContextMenu';
import { Menu } from '@/ui/Menu';
import { RadioGroup } from '@/ui/RadioGroup';
import { Popover } from '@/ui/Popover';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { show } from './support';

const environment = { value: 'environment', name: 'Environment', description: 'Let people work in folders of this machine.' };
const node = { value: 'node', name: 'Node', description: 'Let Orch make environments here.' };

describe('compact names in the page, explanations inside open pickers', () => {
  it('keeps a form name and its supplied explanation visible', () => {
    show(<Choice name={environment.name} description={environment.description} />);
    expect(screen.getByText(environment.name)).toBeVisible();
    expect(screen.getByText(environment.description)).toBeVisible();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('names a form radio by the term and opens supplied help without selecting it', async () => {
    const user = userEvent.setup(), choose = vi.fn(), guide = vi.fn();
    show(<RadioGroup label="Machine roles" value="node" onValueChange={choose} options={[
      { ...environment, help: { label: 'Guide to Environment', onOpen: guide } }, node,
    ]} />);
    const radio = screen.getByRole('radio', { name: 'Environment' });
    expect(radio).toHaveAccessibleDescription(environment.description);
    expect(screen.getByText(environment.description)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Guide to Environment' }));
    expect(guide).toHaveBeenCalledTimes(1);
    expect(choose).not.toHaveBeenCalled();
    expect(screen.getByRole('radio', { name: 'Node' })).toBeChecked();
    await user.click(radio);
    expect(choose).toHaveBeenLastCalledWith('environment');
  });

  it('keeps arrow-key radio navigation and the disabled choice’s reason', async () => {
    const user = userEvent.setup(), choose = vi.fn();
    function Roles() {
      const [value, setValue] = useState('environment');
      return <RadioGroup label="Machine roles" value={value} onValueChange={(next) => { setValue(next); choose(next); }} options={[
        environment, { ...node, disabled: 'Ask the machine owner.' }, { value: 'agent', name: 'Agent', description: 'Run agents here.' },
      ]} />;
    }
    show(<Roles />);
    const off = screen.getByRole('radio', { name: 'Node' });
    expect(off).toBeDisabled();
    expect(off).toHaveAccessibleDescription(`${node.description} Ask the machine owner.`);
    expect(screen.getByText(node.description)).toBeVisible();
    await user.tab();
    expect(screen.getByRole('radio', { name: 'Environment' })).toHaveFocus();
    // Radix defers the focus move; keyup follows that move rather than sharing its keydown task.
    await user.keyboard('{ArrowRight>}');
    try {
      await waitFor(() => {
        expect(choose).toHaveBeenLastCalledWith('agent');
        expect(screen.getByRole('radio', { name: 'Agent' })).toHaveFocus();
        expect(screen.getByRole('radio', { name: 'Agent' })).toBeChecked();
        expect(off).not.toBeChecked();
      });
    } finally { await user.keyboard('{/ArrowRight}'); }
    expect(choose).toHaveBeenCalledExactlyOnceWith('agent');
  });

  it('keeps segments compact even when their shared copy includes descriptions and guide metadata', async () => {
    const user = userEvent.setup(), choose = vi.fn(), guide = vi.fn();
    show(<SegmentedControl label="Machine roles" value="environment" onValueChange={choose} segments={[
      environment, { ...node, help: { label: 'Guide to Node', onOpen: guide } },
    ]} />);
    const choice = screen.getByRole('radio', { name: 'Node' });
    expect(choice).not.toHaveAttribute('aria-describedby');
    expect(screen.queryByText(node.description)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Guide to Node' })).not.toBeInTheDocument();
    expect(guide).not.toHaveBeenCalled();
    expect(choose).not.toHaveBeenCalled();
    await user.click(choice);
    expect(choose).toHaveBeenLastCalledWith('node');
  });

  it('uses named form checkboxes with visible descriptions and keeps disabled choices unchanged', async () => {
    const user = userEvent.setup(), changed = vi.fn(), guide = vi.fn();
    function Roles() {
      const [value, setValue] = useState<string[]>([]);
      return <CheckboxList label="Machine roles" value={value} onValueChange={(next) => { setValue(next); changed(next); }} options={[
        { ...environment, help: { label: 'Guide to Environment', onOpen: guide } }, { ...node, disabled: true },
      ]} />;
    }
    show(<Roles />);
    const group = screen.getByRole('group', { name: 'Machine roles' });
    const choice = within(group).getByRole('checkbox', { name: 'Environment' });
    expect(choice).toHaveAccessibleDescription(environment.description);
    expect(screen.getByText(environment.description)).toBeVisible();
    expect(within(group).getByRole('checkbox', { name: 'Node' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Guide to Environment' }));
    expect(guide).toHaveBeenCalledTimes(1);
    expect(changed).not.toHaveBeenCalled();
    await user.click(screen.getByText(environment.name));
    expect(choice).toBeChecked();
    expect(changed).toHaveBeenLastCalledWith(['environment']);
    await user.click(screen.getByText(environment.name));
    expect(choice).not.toBeChecked();
    expect(changed).toHaveBeenLastCalledWith([]);
  });

  it('keeps an existing checkbox’s native form name and supplied hint', () => {
    const { container } = show(<form><Checkbox name="scope" label="Workspace" hint="Files in this workspace." checked onCheckedChange={vi.fn()} /></form>);
    expect(screen.getByRole('checkbox', { name: 'Workspace' })).toHaveAccessibleDescription('Files in this workspace.');
    expect(screen.getByText('Files in this workspace.')).toBeVisible();
    expect(container.querySelector('input[name="scope"]')).not.toBeNull();
  });

  it.each([
    { language: 'en', picker: 'Choose tags', name: 'Ready', description: 'Defined in Acme.', blocked: 'Urgent', blockedDescription: 'Defined in Research.' },
    { language: 'zh-CN', picker: '选择标签', name: '就绪', description: '定义于 Acme。', blocked: '紧急', blockedDescription: '定义于 Research。' },
  ])('describes explicit checkbox picker options in $language and keeps their names, label clicks and disabled guards', async (words) => {
    const user = userEvent.setup();
    const changed = vi.fn();
    function Tags() {
      const [value, setValue] = useState<string[]>([]);
      return <Popover label={words.picker} trigger={<button>{words.picker}</button>}>
        <CheckboxList label={words.picker} presentation="picker" value={value} onValueChange={(next) => { setValue(next); changed(next); }} options={[
          { value: 'ready', name: words.name, description: words.description },
          { value: 'urgent', name: words.blocked, description: words.blockedDescription, disabled: true },
        ]} />
      </Popover>;
    }
    show(<Tags />);
    expect(screen.queryByText(words.description)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: words.picker }));
    const choice = screen.getByRole('checkbox', { name: words.name });
    expect(choice).toHaveAccessibleDescription(words.description);
    const blocked = screen.getByRole('checkbox', { name: words.blocked });
    expect(blocked).toBeDisabled();
    expect(blocked).toHaveAccessibleDescription(words.blockedDescription);
    await user.click(screen.getByText(words.description));
    expect(choice).toBeChecked();
    expect(changed).toHaveBeenCalledExactlyOnceWith(['ready']);
    await user.click(screen.getByText(words.blockedDescription));
    expect(changed).toHaveBeenCalledTimes(1);
    await user.keyboard('{Escape}');
    expect(screen.queryByText(words.description)).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('keeps ordinary Select names, descriptions, keyboard selection and the name-only selected value', async () => {
    const user = userEvent.setup(), choose = vi.fn();
    show(<Select label="Machine role" value="environment" onValueChange={choose} options={[environment, node]} />);
    const trigger = screen.getByRole('combobox', { name: 'Machine role' });
    expect(trigger).toHaveTextContent('Environment');
    expect(trigger).not.toHaveTextContent(environment.description);
    await user.click(trigger);
    const option = await screen.findByRole('option', { name: 'Node' });
    expect(option).toHaveAccessibleDescription(node.description);
    expect(option).toContainElement(screen.getByText(node.description));
    await waitFor(() => expect(screen.getByRole('option', { name: 'Environment' })).toHaveFocus());
    await user.keyboard('{ArrowDown}');
    await waitFor(() => expect(option).toHaveFocus());
    await user.keyboard('{Enter}');
    expect(choose).toHaveBeenLastCalledWith('node');
  });

  it('takes Tab to a Select choice’s guide outside the listbox, without choosing the highlighted value', async () => {
    const user = userEvent.setup(), choose = vi.fn(), guide = vi.fn();
    show(<Select label="Machine role" value="environment" onValueChange={choose} options={[
      environment, { ...node, help: { label: 'Guide to Node', onOpen: guide } },
    ]} />);
    const trigger = screen.getByRole('combobox', { name: 'Machine role' });
    expect(trigger).toHaveAttribute('aria-haspopup', 'dialog');
    await user.click(trigger);
    const popup = await screen.findByRole('dialog', { name: 'Machine role' });
    const list = within(popup).getByRole('listbox', { name: 'Machine role' });
    await waitFor(() => expect(within(list).getByRole('option', { name: 'Environment' })).toHaveFocus());
    await user.keyboard('{ArrowDown}');
    await waitFor(() => expect(within(list).getByRole('option', { name: 'Node' })).toHaveFocus());
    const help = within(popup).getByRole('button', { name: 'Guide to Node' });
    expect(list).not.toContainElement(help);
    await user.tab();
    expect(help).toHaveFocus();
    await user.tab({ shift: true });
    expect(within(list).getByRole('option', { name: 'Node' })).toHaveFocus();
    await user.keyboard('{F1}');
    expect(guide).toHaveBeenCalledOnce();
    expect(choose).not.toHaveBeenCalled();
    expect(trigger).toHaveTextContent('Environment');
  });

  it('gives checkable menus familiar names and descriptions without guide menu items', async () => {
    const user = userEvent.setup(), changed = vi.fn(), guide = vi.fn();
    show(<Menu label="Roles" trigger={<button>Choose roles</button>} entries={[
      { type: 'check', id: 'environment', name: environment.name, description: environment.description, checked: false, onCheckedChange: changed, help: { label: 'Guide to Environment', onOpen: guide } },
      { type: 'radio', id: 'role', label: 'Primary role', value: 'node', onValueChange: changed, options: [environment, { ...node, help: { label: 'Guide to Node', onOpen: guide } }] },
    ]} />);
    await user.click(screen.getByRole('button', { name: 'Choose roles' }));
    const check = await screen.findByRole('menuitemcheckbox', { name: 'Environment' });
    expect(check).toHaveAccessibleDescription(environment.description);
    expect(screen.getByRole('menuitemradio', { name: 'Node' })).toHaveAccessibleDescription(node.description);
    expect(screen.queryByRole('menuitem', { name: /Guide to/ })).not.toBeInTheDocument();
    await user.keyboard('{Home}');
    await waitFor(() => expect(check).toHaveFocus());
    await user.click(check);
    expect(changed).toHaveBeenLastCalledWith(true);
    expect(guide).not.toHaveBeenCalled();
  });

  it('keeps a disabled Select choice’s guide reachable without making it selectable', async () => {
    const user = userEvent.setup(), choose = vi.fn(), guide = vi.fn();
    show(<Select label="Machine role" value="environment" onValueChange={choose} options={[
      environment, { ...node, disabled: true, help: { label: 'Guide to Node', onOpen: guide } },
    ]} />);
    await user.click(screen.getByRole('combobox', { name: 'Machine role' }));
    const option = await screen.findByRole('option', { name: 'Node' });
    expect(option).toHaveAttribute('aria-disabled', 'true');
    await waitFor(() => expect(screen.getByRole('option', { name: 'Environment' })).toHaveFocus());
    await user.tab();
    expect(screen.getByRole('button', { name: 'Guide to Node' })).toHaveFocus();
    await user.keyboard('{Enter}');
    expect(guide).toHaveBeenCalledOnce();
    expect(choose).not.toHaveBeenCalled();
  });

  it('uses the same words and blocked reason for an ordinary destination menu item', async () => {
    const user = userEvent.setup(), choose = vi.fn(), guide = vi.fn();
    show(<Menu label="Destinations" trigger={<button>Move to</button>} entries={[
      { id: 'node', name: 'Node', description: node.description, disabled: 'Ask the machine owner.', onSelect: choose, help: { label: 'Guide to Node', onOpen: guide } },
    ]} />);
    await user.click(screen.getByRole('button', { name: 'Move to' }));
    const item = await screen.findByRole('menuitem', { name: 'Node' });
    expect(item).toHaveAttribute('aria-disabled', 'true');
    expect(item).toHaveAccessibleDescription(`${node.description} Ask the machine owner.`);
    await user.click(item);
    expect(choose).not.toHaveBeenCalled();
    expect(screen.queryByRole('menuitem', { name: 'Guide to Node' })).not.toBeInTheDocument();
    expect(guide).not.toHaveBeenCalled();
    expect(choose).not.toHaveBeenCalled();
  });

  it('keeps the same names and descriptions in the context menu', async () => {
    show(<ContextMenu label="Roles" entries={[
      { type: 'check', id: 'environment', ...environment, checked: true, onCheckedChange: vi.fn(), help: { label: 'Guide to Environment', onOpen: vi.fn() } },
    ]}><button>Context</button></ContextMenu>);
    fireEvent.contextMenu(screen.getByRole('button', { name: 'Context' }), { clientX: 20, clientY: 20 });
    const item = await screen.findByRole('menuitemcheckbox', { name: 'Environment' });
    expect(item).toBeChecked();
    expect(item).toHaveAccessibleDescription(environment.description);
    expect(screen.queryByRole('menuitem', { name: 'Guide to Environment' })).not.toBeInTheDocument();
  });
});


it('retains compact radio guidance on keyboard focus and keeps the supplied choice behavior', async () => {
  const user = userEvent.setup(), choose = vi.fn();
  show(<RadioGroup presentation="compact" label="Machine roles" value="environment" onValueChange={choose} options={[environment, node]} />);
  expect(document.querySelector('[data-choice-description]')).toBeNull();
  await user.tab();
  const radio = screen.getByRole('radio', { name: environment.name });
  expect(radio).toHaveFocus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent(environment.description);
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('radio', { name: node.name }));
  expect(choose).toHaveBeenLastCalledWith('node');
});

it('retains compact switch guidance on keyboard focus without changing its name or toggle', async () => {
  const user = userEvent.setup(), choose = vi.fn();
  show(<Switch presentation="compact" label="Keep awake" description="Keep this runtime available." checked onCheckedChange={choose} />);
  expect(document.querySelector('[data-choice-description]')).toBeNull();
  await user.tab();
  const toggle = screen.getByRole('switch', { name: 'Keep awake' });
  expect(toggle).toHaveFocus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent('Keep this runtime available.');
  await user.keyboard('{Escape}');
  await user.keyboard(' ');
  expect(choose).toHaveBeenCalledExactlyOnceWith(false);
});
