/** New operators edit explained driver fields, keep unknown values and see the server's own problems. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { checkTemplate } from '@/api/resources';
import { TemplateEditor } from '@/features/resources/templates/TemplateEditor';
import { SchemaForm } from '@/features/resources/templates/SchemaSettings';
import type { Schema } from '@/features/resources/templates/schema';
import { i18n, setLocale } from '@/i18n';
import { Switch } from '@/ui/Switch';
import { show } from './support';

vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), checkTemplate: vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
// The real editor's keyboard, completion, hover and line rendering are exercised in the browser test.
vi.mock('@/ui/CodeView', () => ({ CodeView: ({ text, label, readOnly, onChange }: { text: string; label: string; readOnly?: boolean; onChange?: (next: string) => void }) => <textarea aria-label={label} value={text} readOnly={readOnly} onChange={(event) => onChange?.(event.target.value)} /> }));

const schema: Schema = { type: 'object', additionalProperties: true, properties: {
  image: { type: 'string', title: 'Project image', description: 'An image with the project tools.', default: null, examples: ['node:22-bookworm'] },
  resources: { type: 'object', title: 'Limits', description: 'Limits for a unit.', properties: {
    cores: { type: 'number', title: 'CPU cores', description: 'Raise for parallel work.', default: 4, minimum: 1, maximum: 16, 'x-unit': 'cores' },
  } },
  network: { type: 'object', title: 'Network', description: 'Connections.', properties: { mode: { title: 'Network mode', enum: ['bridge', 'none'], default: 'bridge', description: 'Choose ordinary or Orch-only access.' } } },
  secure: { type: 'boolean', title: 'Keep privileges fixed', description: 'Prevent new privileges.', default: true },
  env: { type: 'object', title: 'Variables', description: 'Plain values.', additionalProperties: { type: 'string' }, default: {} },
  tools: { type: 'array', title: 'Tools', description: 'One tool name per line.', default: [], items: { type: 'string' } },
}, 'x-orch-examples': [{ id: 'starter', title: 'Web starter', description: 'For a web project.', spec: { image: 'node:22-bookworm', resources: { cores: 4 } } }] };

function Form({ start = { image: 'team/dev:1' }, driver = 'sample', onChange = () => undefined }: { start?: Record<string, unknown>; driver?: string; onChange?: (text: string) => void }) {
  const [text, setText] = useState(JSON.stringify(start));
  return <TemplateEditor driver={driver} kind="environment" schema={schema} text={text} onChange={(next) => { setText(next); onChange(next); }} examples />;
}

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  vi.mocked(checkTemplate).mockResolvedValue({ valid: true, errors: [] });
});

it('searches all driver choices without validating typing or materializing the inherited default', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const choices = Array.from({ length: 12 }, (_, index) => `mode-${index + 1}`);
  const declared: Schema = { type: 'object', properties: { mode: { title: 'Driver mode', type: 'string',
    description: 'Choose the driver mode.', enum: choices, default: 'mode-11' } } };
  function Editor() {
    const [text, setText] = useState('{"extension":{"keep":true}}');
    return <TemplateEditor driver="sample" kind="environment" schema={declared} text={text}
      onChange={(next) => { changed(next); setText(next); }} />;
  }
  show(<Editor />);
  await waitFor(() => expect(checkTemplate).toHaveBeenCalledOnce());
  const input = screen.getByRole('combobox', { name: 'Driver mode' });
  expect(input).toHaveValue('mode-11');
  await user.click(input); expect(screen.getAllByRole('option')).toHaveLength(12);
  await user.type(input, 'mode-12');
  // Cross the editor's 350 ms validation pause: query text is not a configuration edit.
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 700)); });
  expect(changed).not.toHaveBeenCalled(); expect(checkTemplate).toHaveBeenCalledOnce();
  await user.keyboard('{Escape}'); expect(input).toHaveValue('mode-11');
  await user.click(input); await user.type(input, 'mode-12'); await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledOnce();
  expect(JSON.parse(String(changed.mock.calls[0]?.[0]))).toEqual({ extension: { keep: true }, mode: 'mode-12' });
  await waitFor(() => expect(checkTemplate).toHaveBeenCalledTimes(2));
  expect(vi.mocked(checkTemplate).mock.calls[1]?.[0].spec).toEqual({ extension: { keep: true }, mode: 'mode-12' });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.editor.menuOf', { name: 'Driver mode' }) }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t('templates.editor.reset') }));
  expect(JSON.parse(String(changed.mock.calls.at(-1)?.[0]))).toEqual({ extension: { keep: true } });
  expect(input).toHaveValue('mode-11');
});

it('renders a Docker schema with untranslated fields and enum values using its own English', async () => {
  const user=userEvent.setup();
  const future:Schema={type:'object',properties:{future_choice:{type:'string',title:'Driver choice',description:'An explanation shipped by the driver.',enum:['new-driver-choice'],default:'new-driver-choice'}}};
  expect(i18n.exists('templates.fields.future_choice.title')).toBe(false);
  show(<SchemaForm schema={future} spec={{future_choice:'new-driver-choice'}} driver="docker" onChange={()=>undefined} />);
  const choice=screen.getByRole('combobox',{name:'Driver choice'});
  expect(choice).toHaveValue('new-driver-choice');
  expect(screen.getByText('An explanation shipped by the driver.')).toBeVisible();
  await user.click(choice);
  expect(screen.getByRole('option',{name:'new-driver-choice'})).toBeInTheDocument();
});

it.each([
  {locale:'en' as const,field:'Outgoing connections',name:'Restricted',description:"Reach Orch and host names in the place's allow-list."},
  {locale:'zh-CN' as const,field:'对外连接',name:'受限',description:'仅访问 Orch 和所在位置允许列表中的主机名。'},
])('names an enum choice before its separate description in $locale',async({locale,field,name,description})=>{
  await setLocale(locale);const user=userEvent.setup();const changed=vi.fn();
  const explained:Schema={type:'object',properties:{network:{type:'object',properties:{egress:{enum:['open','restricted'],title:'Outgoing connections',description:'A driver explanation.'}}}}};
  show(<SchemaForm driver="docker" schema={explained} spec={{network:{egress:'open',future_setting:'kept'}}} onChange={changed} />);
  const select=screen.getByRole('combobox',{name:field});
  await user.click(select);
  const option=screen.getByRole('option',{name});
  expect(option).toHaveAccessibleName(name);expect(option).toHaveAccessibleDescription(description);
  expect(within(option).getByText(description)).toBeVisible();
  await user.click(option);
  expect(changed).toHaveBeenLastCalledWith({network:{egress:'restricted',future_setting:'kept'}});
});

it('keeps examples in one compact picker and explains each only inside it',async()=>{
  show(<Form />);
  const example=screen.getByRole('button',{name:'Start from an example'});
  expect(screen.queryByText('For a web project.')).not.toBeInTheDocument();
  expect(example).not.toHaveClass('h-auto');
  await userEvent.click(example);
  const choice=screen.getByRole('menuitem',{name:'Web starter'});
  expect(choice).toHaveAccessibleDescription('For a web project.');
});

it('keeps a switch explanation and opens supplied help without changing its state',async()=>{
  const user=userEvent.setup();const changed=vi.fn();const guide=vi.fn();
  show(<Switch name="JSON" description="Edit the same configuration as JSON." help={{label:'About JSON',onOpen:guide}} checked={false} onCheckedChange={changed} />);
  const control=screen.getByRole('switch',{name:'JSON'});
  expect(control).toHaveAccessibleDescription('Edit the same configuration as JSON.');
  expect(screen.getByText('Edit the same configuration as JSON.')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'About JSON'}));
  expect(guide).toHaveBeenCalledTimes(1);expect(changed).not.toHaveBeenCalled();
  await user.click(control);expect(changed).toHaveBeenLastCalledWith(true);
});

it('keeps an existing switch label and its supplied legacy hint',()=>{
  show(<Switch label="Enabled" hint="Allow new units." checked={false} onCheckedChange={()=>undefined} />);
  expect(screen.getByRole('switch',{name:'Enabled'})).toHaveAccessibleDescription('Allow new units.');
  expect(screen.getByText('Allow new units.')).toBeVisible();
});

it('uses driver choice names for an empty enum value and keeps the original value when choosing another',async()=>{
  const user=userEvent.setup();const changed=vi.fn();
  const declared:Schema={type:'object',properties:{mode:{type:'string',title:'Mode',enum:['','direct'],'x-choices':{'':{name:'Empty',description:'Do not add an extra mode.'},direct:{name:'Direct',description:'Connect directly.'}}}}};
  show(<SchemaForm schema={declared} spec={{mode:''}} driver="custom" onChange={changed} />);
  const select=screen.getByRole('combobox',{name:'Mode'});expect(select).toHaveValue('Empty');
  await user.click(select);await user.click(screen.getByRole('option',{name:'Direct'}));
  expect(changed).toHaveBeenLastCalledWith({mode:'direct'});
});

it('draws the plugin fields and defaults as controls without writing defaults into the configuration', async () => {
  const changed = vi.fn(); show(<Form onChange={changed} />);
  expect(screen.getByLabelText('Project image')).toHaveValue('team/dev:1');
  const user=userEvent.setup();
  expect(screen.getByText('An image with the project tools.')).toBeVisible();
  const cpu = screen.getByRole('spinbutton', { name: 'CPU cores' });
  expect(cpu).not.toHaveAttribute('placeholder');
  expect(cpu).toHaveAttribute('min', '1'); expect(cpu).toHaveAttribute('max', '16');
  expect(cpu).toHaveValue(4);
  expect(screen.getByRole('switch', { name: 'Keep privileges fixed' })).toBeChecked();
  expect(screen.getByRole('combobox', { name: 'Network mode' })).toBeInTheDocument();
  expect(screen.queryByLabelText('Tools · one per line')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Edit Tools'}));
  expect(screen.getByLabelText('Tools · one per line')).toBeVisible();
  await waitFor(() => expect(checkTemplate).toHaveBeenCalled());
  expect(vi.mocked(checkTemplate).mock.calls.at(-1)?.[0].spec).toEqual({ image: 'team/dev:1' });
  expect(changed).not.toHaveBeenCalled();
  expect(screen.getAllByText('Network',{exact:true})).toHaveLength(1);
});

it('puts server problems beside the field and resetting removes only that override', async () => {
  const user = userEvent.setup(); const changed = vi.fn();
  vi.mocked(checkTemplate).mockResolvedValue({ valid: false, errors: [{ pointer: '/resources/cores', message: 'Choose at least one core.' }] });
  show(<Form start={{ image: 'team/dev:1', resources: { cores: 0, future_limit: 9 }, extension: { keep: true } }} onChange={changed} />);
  const cpu = screen.getByRole('spinbutton', { name: 'CPU cores' });
  await waitFor(() => expect(cpu).toHaveAttribute('aria-invalid', 'true'));
  expect(cpu).toHaveAccessibleDescription('Choose at least one core.');
  await user.click(screen.getByRole('button', { name: i18n.t('settings.editor.menuOf',{name:'CPU cores'}) }));
  expect(screen.getByText('Default: 4 cores')).toBeVisible();
  await user.click(screen.getByRole('menuitem', { name: 'Reset to default' }));
  expect(cpu).toHaveValue(4);
  expect(JSON.parse(String(changed.mock.calls.at(-1)?.[0]))).toEqual({ image: 'team/dev:1', resources: { future_limit: 9 }, extension: { keep: true } });
});

it('keeps unknown nested properties through form edits and a JSON round trip', async () => {
  const user = userEvent.setup(); const changed = vi.fn();
  show(<Form start={{ image: 'team/dev:1', resources: { cores: 6, future_limit: 9 }, extension: { keep: true } }} onChange={changed} />);
  await user.click(screen.getByRole('button',{name:'Edit Limits: Other settings (JSON)'}));
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  expect(screen.getAllByLabelText('Other settings (JSON)')).toHaveLength(2);
  fireEvent.change(screen.getByRole('spinbutton', { name: 'CPU cores' }), { target: { value: '8' } });
  await user.click(screen.getByRole('switch', { name: 'JSON' }));
  const editor = screen.getByLabelText('Configuration (JSON)');
  const next = { image: 'team/dev:2', resources: { cores: 12, future_limit: 9 }, extension: { keep: true }, tools: ['compiler', 'git'] };
  fireEvent.change(editor, { target: { value: JSON.stringify(next) } });
  await user.click(screen.getByRole('switch', { name: 'JSON' }));
  expect(screen.getByLabelText('Project image')).toHaveValue('team/dev:2');
  expect(screen.getByRole('spinbutton', { name: 'CPU cores' })).toHaveValue(12);
  await user.click(screen.getByRole('button',{name:'Edit Tools'}));
  expect(screen.getByLabelText('Tools · one per line')).toHaveValue('compiler\ngit');
  await user.click(screen.getByRole('switch', { name: 'JSON' }));
  expect(JSON.parse(screen.getByLabelText<HTMLTextAreaElement>('Configuration (JSON)').value)).toEqual(next);
});

it('keeps an invalid JSON draft visible and refuses to switch away until it is fixed', async () => {
  const user = userEvent.setup(); show(<Form start={{ image: 'team/dev:1', extension: { keep: true } }} />);
  await user.click(screen.getByRole('button',{name:'Edit Other settings (JSON)'}));
  fireEvent.change(screen.getByLabelText('Other settings (JSON)'), { target: { value: '{' } });
  expect(screen.getByLabelText('Other settings (JSON)')).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByRole('switch', { name: 'JSON' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Other settings (JSON)'), { target: { value: '{"extension":{"keep":true}}' } });
  await user.click(screen.getByRole('switch', { name: 'JSON' }));
  fireEvent.change(screen.getByLabelText('Configuration (JSON)'), { target: { value: '{' } });
  expect(screen.getByRole('switch', { name: 'JSON' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Configuration (JSON)'), { target: { value: '{"image":"team/dev:1"}' } });
  await user.click(screen.getByRole('switch', { name: 'JSON' }));
  expect(screen.getByLabelText('Project image')).toHaveValue('team/dev:1');
});

it('starts from a whole driver example and edits key-value variables without splitting values', async () => {
  const user = userEvent.setup(); const changed = vi.fn(); show(<Form onChange={changed} />);
  await user.click(screen.getByRole('button', { name: 'Start from an example' }));
  await user.click(screen.getByRole('menuitem', { name: 'Web starter' }));
  expect(screen.getByLabelText('Project image')).toHaveValue('node:22-bookworm');
  const variables = screen.getByRole('region', { name: 'Variables' });
  await user.click(within(variables).getByRole('button',{name:'Edit Variables'}));
  await user.click(within(variables).getByRole('button', { name: 'Add variable' }));
  fireEvent.change(within(variables).getByLabelText('Name'), { target: { value: 'BUILD_FLAGS' } });
  fireEvent.change(within(variables).getByLabelText('Value'), { target: { value: 'a b $HOME --literal' } });
  expect(JSON.parse(String(changed.mock.calls.at(-1)?.[0]))).toMatchObject({ image: 'node:22-bookworm', resources: { cores: 4 }, env: { BUILD_FLAGS: 'a b $HOME --literal' } });
});

it('uses Chinese field help for Docker while a plugin keeps its own English explanation', async () => {
  await setLocale('zh-CN');
  const { unmount } = show(<SchemaForm schema={schema} spec={{ image: 'node:22-bookworm' }} driver="docker" onChange={() => undefined} />);
  expect(screen.getByLabelText('镜像引用')).toBeInTheDocument();
  expect(screen.getByText(/从镜像仓库拉取/)).toBeInTheDocument(); unmount();
  show(<SchemaForm schema={schema} spec={{ image: 'node:22-bookworm' }} driver="sample" onChange={() => undefined} />);
  expect(screen.getByLabelText('Project image')).toBeInTheDocument();
  expect(screen.getByText('An image with the project tools.')).toBeInTheDocument();
  await setLocale('en');
});
