/** Driver metadata is complete; choosing it changes a local template draft, never validates or creates while searching. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { NewTemplateDialog } from '@/features/resources/templates/NewTemplate';
import type { DriverInfo, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { id } from '../doubles/fixtures';
import { detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ drivers: vi.fn<() => Promise<DriverInfo[]>>(), create: vi.fn<typeof Resources.createTemplate>(), check: vi.fn<typeof Resources.checkTemplate>(), close: vi.fn(), navigate: vi.fn() }));
const HOME_ID = id(72, 1);
const drivers: DriverInfo[] = Array.from({ length: 13 }, (_, index) => {
  const number = index + 1;
  const schema = { type: 'object', required: ['endpoint'], properties: { endpoint: { type: 'string', title: `Driver endpoint ${number}`, default: `https://driver.example.test/${number}` } } };
  return { name: `driver-${number}`, title: `Declared driver ${number}`, hosts: number, descriptor: { description: `Run through driver route ${number}.`, spec_schemas: { environment: schema, provider: schema } } };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), createTemplate: calls.create, checkTemplate: calls.check,
  driversQuery: { queryKey: ['metadata-driver-catalogue'], queryFn: calls.drivers } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => calls.navigate }));
vi.mock('@/features/resources/HomePicker', () => ({ HomePicker: ({ value, onChange, disabled }: Pick<HomePickerProps, 'value' | 'onChange' | 'disabled'>) =>
  <select aria-label="Fixture home" value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)}><option value="">Choose</option><option value={HOME_ID}>Workshop</option></select> }));
// Host eligibility is a separate resource reader; this test does not turn its window into a catalogue.
vi.mock('@/features/resources/templates/TemplateHosts', () => ({ TemplateHosts: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

beforeEach(async () => { await setLocale('en'); vi.clearAllMocks(); calls.drivers.mockResolvedValue(drivers); });
afterEach(() => { cleanup(); });

it('retains the draft while locating a late driver, then uses that driver’s declared defaults in one explicit creation', async () => {
  let accept: (value: Template) => void = () => undefined;
  calls.create.mockReturnValueOnce(new Promise<Template>((resolve) => { accept = resolve; }));
  const user = userEvent.setup(); show(<NewTemplateDialog onClose={calls.close} />);
  const picker = await screen.findByRole('combobox', { name: i18n.t('templates.new.driver') });
  expect(picker).toHaveValue('Declared driver 1');
  await user.type(screen.getByRole('textbox', { name: i18n.t('templates.new.name') }), 'Native template');
  await user.selectOptions(screen.getByRole('combobox', { name: 'Fixture home' }), HOME_ID);
  const first = screen.getByRole('textbox', { name: 'Driver endpoint 1' });
  await user.clear(first); await user.type(first, 'https://draft.example.test');
  await user.click(picker); expect(within(screen.getByRole('listbox')).getAllByRole('option')).toHaveLength(13);
  await user.type(picker, 'driver-13');
  const description = `Run through driver route 13. ${i18n.t('templates.new.driverHosts', { count: 13 })}`;
  expect(screen.getByText(description, { exact: true })).toBeVisible(); expect(first).toHaveValue('https://draft.example.test');
  expect(calls.drivers).toHaveBeenCalledOnce(); expect(calls.check).not.toHaveBeenCalled(); expect(calls.create).not.toHaveBeenCalled();
  await user.keyboard('{Escape}'); expect(picker).toHaveValue('Declared driver 1'); expect(first).toHaveValue('https://draft.example.test');
  await user.click(picker); await user.type(picker, 'driver-13'); await user.keyboard('{Enter}');
  expect(picker).toHaveValue('Declared driver 13'); expect(screen.getByRole('textbox', { name: 'Driver endpoint 13' })).toHaveValue('https://driver.example.test/13');
  expect(calls.create).not.toHaveBeenCalled(); expect(calls.check).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('templates.new.submit') }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledWith({ name: 'Native template', description: '', driver: 'driver-13', home_id: HOME_ID,
    kind: 'environment', spec: { endpoint: 'https://driver.example.test/13' }, access: 'only_me' }, expect.any(String)));
  expect(calls.create).toHaveBeenCalledOnce(); expect(picker).toBeDisabled();
  await act(async () => { accept({ ...detailTemplate, id: id(72, 2), name: 'Native template', driver: 'driver-13', home: { id: HOME_ID, kind: 'org', name: 'Workshop', redacted: false }, org_id: HOME_ID,
    spec: { endpoint: 'https://driver.example.test/13' } }); await Promise.resolve(); });
  await waitFor(() => expect(calls.close).toHaveBeenCalledOnce());
  expect(calls.drivers).toHaveBeenCalledOnce(); expect(calls.check).not.toHaveBeenCalled();
});
