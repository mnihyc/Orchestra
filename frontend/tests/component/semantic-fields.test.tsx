/** Semantic search fields have short localized labels and keep the provider before the model and dimensions. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { EmbeddingsChoice, SemanticTuning, SemanticTuningSummary } from '@/features/settings/EmbeddingsChoice';
import { shapeOf } from '@/features/settings/settingsModel';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { show } from './support';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { useAppearance } from '@/lib/appearance';
import { SettingSummary } from '@/features/settings/SettingSummary';

vi.mock('@/features/settings/ProviderChoice', () => ({
  ProviderDefaultChoice: ({ label }: ValueControlProps) => <select aria-label={label}><option /></select>,
  ProviderDefaultName: () => null,
}));

it.each(['en', 'zh-CN'] as const)('names the embeddings and tuning controls in %s without exposing schema member names', async (locale) => {
  const previous = currentLocale();
  await setLocale(locale);
  const embeddings = SETTINGS_REGISTRY.find((def) => def.key === 'search.semantic.embeddings');
  const tuning = SETTINGS_REGISTRY.find((def) => def.key === 'search.semantic.tuning');
  if (embeddings === undefined || tuning === undefined) throw new Error('semantic settings are registered');
  const view = show(<EmbeddingsChoice shape={shapeOf(embeddings.schema)} label={i18n.t('settingRegistry.search.semantic.embeddings.title')} onChange={vi.fn()}
    value={{ provider_id: '01990000-0000-7000-8000-000000000001', model_name: 'example-model', dimensions: 256, query_prefix: 'query: ', document_prefix: 'text: ' }} />);
  try {
    const provider = screen.getByRole('combobox', { name: i18n.t('admin.semantic.provider') });
    const model = screen.getByRole('textbox', { name: i18n.t('admin.semantic.fields.model') });
    const dimensions = screen.getByRole('spinbutton', { name: i18n.t('admin.semantic.fields.dimensions') });
    expect(provider.compareDocumentPosition(model) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(model.compareDocumentPosition(dimensions) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole('textbox', { name: i18n.t('admin.semantic.fields.queryPrefix') })).toHaveValue('query: ');
    expect(screen.getByRole('textbox', { name: i18n.t('admin.semantic.fields.documentPrefix') })).toHaveValue('text: ');
    view.unmount();
    const tuningView = show(<SemanticTuning shape={shapeOf(tuning.schema)} value={tuning.default} label={i18n.t('settingRegistry.search.semantic.tuning.title')} onChange={vi.fn()} />);
    try {
      expect(screen.getByRole('spinbutton', { name: i18n.t('admin.semantic.fields.chunkTokens') })).toHaveValue(400);
      expect(screen.getByRole('spinbutton', { name: i18n.t('admin.semantic.fields.queryWait') })).toHaveValue(3);
    } finally { tuningView.unmount(); }
  } finally {
    view.unmount();
    await setLocale(previous);
  }
});

it('keeps durations and counts formatted in a tuning summary',()=>{
  const {locale,timeZone}=useAppearance.getState();
  const view=show(<SemanticTuningSummary value={{interval_secs:10,query_wait_secs:3,candidates:2500}}/>);
  expect(view.container).toHaveTextContent(formatSpan(10000,{locale,timeZone}));
  expect(view.container).toHaveTextContent(formatSpan(3000,{locale,timeZone}));
  expect(view.container).toHaveTextContent(formatNumber(2500,locale));
  view.unmount();
});

it.each(['en','zh-CN'] as const)('names the per-request word budget in summaries in %s',async(locale)=>{
  const previous=currentLocale();
  await setLocale(locale);
  const view=show(<SemanticTuningSummary value={{request_windows:2}}/>);
  try {
    expect(view.container).toHaveTextContent(i18n.t('searchTuning.fields.requestWindows'));
    expect(view.container).toHaveTextContent(formatNumber(2,locale));
    expect(view.container).not.toHaveTextContent('request_windows');
  } finally {view.unmount();await setLocale(previous);}
});

it.each(['en','zh-CN'] as const)('keeps nested word limits localized and removes only the chosen override in %s',async(locale)=>{
  const previous=currentLocale();
  await setLocale(locale);
  const def=SETTINGS_REGISTRY.find((entry)=>entry.key==='search.word_tuning');
  if(def===undefined)throw new Error('Word retrieval is registered');
  const changed=vi.fn();
  const view=show(<SemanticTuning shape={shapeOf(def.schema)} label="Word retrieval"
    value={{scope_candidates:30000,tiers:{title:5}}} onChange={changed}/>);
  try {
    expect(screen.getByRole('spinbutton',{name:i18n.t('searchTuning.fields.scopeRows')})).toHaveValue(30000);
    expect(screen.getByRole('spinbutton',{name:i18n.t('searchTuning.fields.title')})).toHaveValue(5);
    const title=i18n.t('settings.editor.memberSet',{name:i18n.t('searchTuning.fields.title')});
    await userEvent.click(screen.getByRole('checkbox',{name:title}));
    expect(changed).toHaveBeenLastCalledWith({scope_candidates:30000,tiers:{}});
  } finally {view.unmount();await setLocale(previous);}
});

it.each(['en','zh-CN'] as const)('shows an exact-scope zero as Off and translates graph scan summaries in %s',async(locale)=>{
  const previous=currentLocale();
  await setLocale(locale);
  const def=SETTINGS_REGISTRY.find((entry)=>entry.key==='search.semantic.tuning');
  if(def===undefined)throw new Error('Semantic tuning is registered');
  const value={...def.default as Record<string,unknown>,exact_filter_candidates:0,iterative_scan:'strict_order'};
  const view=show(<SemanticTuning shape={shapeOf(def.schema)} value={value} label="Indexing and ranking" onChange={vi.fn()}/>);
  try {
    const allowance=i18n.t('admin.semantic.fields.exactFilter');
    const off=screen.getByRole('textbox',{name:allowance});
    expect(off).toHaveValue(i18n.t('common.off'));
    await userEvent.click(off);
    expect(screen.getByRole('spinbutton',{name:allowance})).toHaveValue(0);
    expect(screen.getByText(i18n.t('common.off'),{exact:true})).toBeVisible();
    await userEvent.tab();
    expect(screen.getByRole('textbox',{name:allowance})).toHaveValue(i18n.t('common.off'));
    view.rerender(<SemanticTuningSummary value={{exact_filter_candidates:0,iterative_scan:'strict_order',ef_search:200}}/>);
    expect(view.container).toHaveTextContent(i18n.t('common.off'));
    expect(view.container).toHaveTextContent(i18n.t('choiceCatalog.searchScan.strict_order.name'));
    expect(view.container).not.toHaveTextContent('strict_order');
  } finally {view.unmount();await setLocale(previous);}
});

it.each(['en', 'zh-CN'] as const)('keeps the disk share’s percent and an unset embeddings choice readable in %s', async (locale) => {
  const previous = currentLocale();
  await setLocale(locale);
  const disk = SETTINGS_REGISTRY.find((def) => def.key === 'search.semantic.max_disk_share');
  if (disk === undefined) throw new Error('The disk share is registered');
  const view = show(<><SettingSummary def={disk} value={100} /><SettingSummary def={{key: 'search.semantic.embeddings'}} value={null} /></>);
  try {
    expect(screen.getByText(`${formatNumber(100, locale)} %`, {exact: true})).toBeVisible();
    expect(screen.getByText(i18n.t('admin.semantic.notSet'), {exact: true})).toBeVisible();
    expect(screen.queryByText(i18n.t('settings.compact.defaultNone'), {exact: true})).toBeNull();
    view.rerender(<SettingSummary def={disk} value={null} />);
    expect(screen.getByText(i18n.t('admin.semantic.noLimit'), {exact: true})).toBeVisible();
  } finally { view.unmount(); await setLocale(previous); }
});
