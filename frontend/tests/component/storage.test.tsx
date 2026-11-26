/** Storage keeps one report model, shared person/reference cards and explicit unknown values. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type { StorageKind, StorageQuery, StorageReport } from '@/generated/api-types';
import { Storage } from '@/features/storage/Storage';
import { patchStorageSearch, type StorageSearch } from '@/features/storage/search';
import { i18n, setLocale } from '@/i18n';
import { storageReport } from '../doubles/storage-report';
import { show, tableDataRows } from './support';
import { ApiError, client } from '@/api/client';
import { PageContext, type PageNavigation } from '@/features/workbench/pageContext';
import { Route as StorageRoute } from '@/routes/storage/index';
import { createRootRoute } from '@tanstack/react-router';

const read=vi.hoisted(()=>vi.fn<(query:StorageQuery)=>StorageReport|undefined>());
vi.mock('@/api/storage',()=>({storagePagesQuery:(query:StorageQuery)=>({queryKey:['storage-test',query],initialPageParam:null as string|null,queryFn:()=>Promise.resolve(read(query) ?? storageReport(query)),getNextPageParam:()=>null})}));
vi.mock('@/features/access/AdminMode',async (loadOriginal) => ({ ...await loadOriginal<object>(),useManagementAvailable:()=>false,useAdminMode:()=>({data:{as_admin:false},isSuccess:true}),AdminModeEnableAction:()=>null}));
vi.mock('@/shell/screen',async (loadOriginal) => ({ ...await loadOriginal<object>(),useScreenHeading:()=>undefined}));
vi.mock('@/features/help/open',async (loadOriginal) => ({ ...await loadOriginal<object>(),useHelpOpener:()=>vi.fn()}));

afterEach(async()=>{cleanup();read.mockReset();document.documentElement.removeAttribute('data-theme');await setLocale('en');});

it('keeps a refused saved storage scope intact while reporting the current default without another read for Details', async () => {
  await setLocale('en');
  // File routes receive their parent/path from the generated tree before the real page adapter reads them.
  const root = createRootRoute(); root.init(0);
  Object.assign(StorageRoute.options, { path: '/storage/', getParentRoute: () => root }); StorageRoute.init(0);
  const before = client.session.getState(), user = 'storage-fallback-reader', key = `orch.${user}.choices.storage.filters`;
  const wanted = { scope: 'node:00000000-0000-4000-8000-000000000001', kind: 'image' }, saved = JSON.stringify(JSON.stringify(wanted));
  localStorage.setItem(key, saved); client.session.setState({ status: 'signed_in', userId: user, readerVersion: (before.readerVersion ?? 0) + 1 });
  read.mockImplementation((query) => { if (query.scope === wanted.scope) throw new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: 'Unavailable', detail: 'Synthetic unavailable scope', request_id: 'synthetic-storage' }, null); return storageReport(query); });
  const Screen = StorageRoute.options.component ?? (() => { throw new Error('The real storage route owns this fallback'); })();
  await Screen.preload?.();
  function Hosted() {
    const [search, setSearch] = useState<Record<string, unknown>>({});
    const navigate = (next: PageNavigation) => { setSearch((before) => typeof next.search === 'function' ? next.search(before) : next.search === true || next.search === undefined ? before : next.search); return Promise.resolve(); };
    return <PageContext value={{ id: 'storage-fallback', target: { route: StorageRoute.id, pathname: '/storage', params: {}, search, hash: '' }, navigate }}><Screen /></PageContext>;
  }
  const view = show(<Hosted />);
  try {
    const fragment = await screen.findByText('Using default · Saved choice unavailable');
    await screen.findByText('Workspace recovery'); expect(read).toHaveBeenCalledTimes(2); expect(localStorage.getItem(key)).toBe(saved);
    const owner = fragment.parentElement; if (owner === null) throw new Error('The current storage filters own their facts');
    await userEvent.click(within(owner).getByRole('button', { name: i18n.t('common.details') }));
    expect(document.querySelector('[data-fact-disclosure-body]')).toHaveTextContent('Default');
    expect(document.querySelector('[data-fact-disclosure-body]')).not.toHaveTextContent(wanted.scope);
    expect(read).toHaveBeenCalledTimes(2); expect(localStorage.getItem(key)).toBe(saved);
  } finally { view.unmount(); localStorage.removeItem(key); client.session.setState(before); }
});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it.each<{ kinds: StorageKind[] }>([
  { kinds: ['volume', 'unit', 'network'] },
  { kinds: ['image', 'build'] },
  { kinds: ['tool_version', 'kept_layer'] },
])(`names and copies $kinds without changing cleanup selection in ${locale}/${theme}`,async({kinds})=>{
  await setLocale(locale);document.documentElement.setAttribute('data-theme',theme);
  const report=storageReport(),base=report.items[0];
  if(base===undefined)throw new Error('The fixture has a native inventory record.');
  report.items=kinds.map((kind,index)=>({...base,id:`native-${index}`,kind,name:`orch-${kind}-00000000-0000-4000-8000-000000000001`}));
  read.mockReturnValue(report);
  const user=userEvent.setup(),open=vi.fn();
  show(<Storage search={{}} onSearch={vi.fn()} onCleanup={open} />);
  const table=await screen.findByRole('table',{name:i18n.t('storage.records')});
  const rows=tableDataRows(table);
  expect(rows).toHaveLength(kinds.length);
  for(const [index,row] of rows.entries()){
    const record=report.items[index];
    if(record===undefined)throw new Error('Every native record has a displayed row.');
    expect(row).toHaveTextContent(i18n.t(`storage.kinds.${record.kind}`));
    expect(row.outerHTML).not.toContain(record.name);
    await user.click(within(row).getByRole('button',{name:i18n.t('common.copyId')}));
    expect(await navigator.clipboard.readText()).toBe(record.name);
    await user.click(within(row).getByRole('checkbox',{name:i18n.t('storage.select',{name:i18n.t(`storage.kinds.${record.kind}`)})}));
  }
  await user.click(screen.getByRole('button',{name:i18n.t('storage.clean')}));
  expect(open).toHaveBeenCalledExactlyOnceWith(report.items.map((row)=>row.id));
});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it(`shows measured and unknown retained data with its one guide at ${locale}/${theme}`,async()=>{
  await setLocale(locale);document.documentElement.setAttribute('data-theme',theme);
  show(<Storage search={{}} onSearch={vi.fn()} />);
  expect(await screen.findByText('Workspace recovery')).toBeVisible();
  expect(screen.getByRole('table',{name:i18n.t('storage.records')})).toHaveTextContent(i18n.t('storage.kinds.image'));
  const locations=screen.getAllByText('Offline host');
  expect(locations).toHaveLength(2);
  for(const location of locations) expect(location).toBeVisible();
  const figures=screen.getByRole('list',{name:locale==='en'?'Storage figures':'存储统计'});
  expect(figures).toHaveTextContent('≥4 KiB');
  expect(screen.getByRole('button',{name:locale==='en'?'Storage cleanup':'存储清理'})).toHaveAttribute('data-help-heading','storage-report');
  expect(read).toHaveBeenCalledTimes(1);
});

it('a breakdown filters the entire report through its server request and returns exact filtered figures',async()=>{
  const user=userEvent.setup();
  function Parent(){const [search,setSearch]=useState<StorageSearch>({});return <Storage search={search} onSearch={(patch)=>setSearch((before)=>patchStorageSearch(before,patch))} />;}
  show(<Parent />);
  await screen.findByText('Workspace recovery');
  const table=screen.getByRole('table',{name:'Kind'});
  await user.click(within(table).getByRole('button',{name:'Image'}));
  await waitFor(()=>expect(read).toHaveBeenLastCalledWith({kind:'image'}));
  await waitFor(()=>expect(screen.queryByText('Workspace recovery')).not.toBeInTheDocument());
  expect(screen.getByRole('list',{name:'Storage figures'})).toHaveTextContent('4 KiB');
  expect(screen.getByRole('button',{name:'Clear filters'})).toBeVisible();
});

it('the dependency card lists permitted names and preserves the complete private count',async()=>{
  const user=userEvent.setup();show(<Storage search={{}} onSearch={vi.fn()} />);
  const name=await screen.findByText('Workspace recovery');
  const row=name.closest('tr');if(row===null)throw new Error('a retained item is a table row');
  await user.click(within(row).getByRole('button',{name:'Kept by'}));
  expect(await screen.findByText('Bound threads · 3')).toBeVisible();
  expect(screen.getByText('Repair redirects')).toBeVisible();
  expect(screen.getByText('and 2 you cannot see')).toBeVisible();
  expect(read).toHaveBeenCalledTimes(1);
});

it('keeps a native dependency handle behind Copy ID when opening Kept by',async()=>{
  const report=storageReport(),base=report.items[0];
  if(base===undefined)throw new Error('The fixture has a native inventory record.');
  if(base.facts===null)throw new Error('The fixture has recorded native dependencies.');
  const handle='orch-unit-00000000-0000-4000-8000-000000000001';
  report.items=[{...base,facts:{...base.facts,dependents:[{relation:'native_dependency',count:1,hidden:0,until:null,visible:[{object:null,name:handle,relation:'native_dependency',until:null}]}]}}];
  read.mockReturnValue(report);
  const user=userEvent.setup();show(<Storage search={{}} onSearch={vi.fn()} />);
  await user.click(await screen.findByRole('button',{name:i18n.t('storage.keptBy')}));
  const card=await screen.findByRole('dialog',{name:i18n.t('storage.keptBy')});
  expect(card).toHaveTextContent(i18n.t('storage.cleanup.itemUnknown'));
  expect(card.outerHTML).not.toContain(handle);
  await user.click(within(card).getByRole('button',{name:i18n.t('common.copyId')}));
  expect(await navigator.clipboard.readText()).toBe(handle);
});

it('selection sends only the chosen opaque records to the common cleanup opener',async()=>{
  const user=userEvent.setup(),open=vi.fn();show(<Storage search={{}} onSearch={vi.fn()} onCleanup={open} />);
  await user.click(await screen.findByRole('checkbox',{name:'Select Image'}));
  await user.click(screen.getByRole('button',{name:'Clean up…'}));
  expect(open).toHaveBeenCalledExactlyOnceWith(['image']);
});

it('finds a late retained record through native text search while keeping its cleanup selection',async()=>{
  const original=storageReport(),base=original.items.find((row)=>row.name==='Workspace recovery');if(base===undefined)throw new Error('a retained record is present');
  read.mockImplementation((query)=>({...original,items:query.q?[{...base,id:'late',name:'Late retained work'}]:[base]}));
  const open=vi.fn(),user=userEvent.setup();show(<Storage search={{}} onSearch={vi.fn()} onCleanup={open}/>);
  await screen.findByText('Workspace recovery');
  await user.click(screen.getByRole('checkbox',{name:i18n.t('storage.select',{name:'Workspace recovery'})}));
  await user.type(screen.getByRole('searchbox'),'Late');
  await waitFor(()=>expect(read).toHaveBeenLastCalledWith(expect.objectContaining({q:'Late'})));
  expect(await screen.findByText('Late retained work')).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('storage.clean')}));
  expect(open).toHaveBeenCalledExactlyOnceWith([base.id]);
});
