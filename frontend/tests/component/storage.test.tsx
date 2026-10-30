/** Storage keeps one report model, shared person/reference cards and explicit unknown values. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type { StorageQuery } from '@/generated/api-types';
import { Storage } from '@/features/storage/Storage';
import { patchStorageSearch, type StorageSearch } from '@/features/storage/search';
import { setLocale } from '@/i18n';
import { storageReport } from '../doubles/storage-report';
import { show } from './support';

const read=vi.hoisted(()=>vi.fn<(query:StorageQuery)=>void>());
vi.mock('@/api/storage',()=>({storagePagesQuery:(query:StorageQuery)=>({queryKey:['storage-test',query],initialPageParam:null as string|null,queryFn:()=>{read(query);return Promise.resolve(storageReport(query));},getNextPageParam:()=>null})}));
vi.mock('@/features/access/AdminMode',()=>({useManagementAvailable:()=>false,useAdminMode:()=>({data:{as_admin:false},isSuccess:true}),AdminModeEnableAction:()=>null}));
vi.mock('@/shell/screen',()=>({useScreenHeading:()=>undefined}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));

afterEach(async()=>{cleanup();read.mockClear();document.documentElement.removeAttribute('data-theme');await setLocale('en');});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it(`shows measured and unknown retained data with its one guide at ${locale}/${theme}`,async()=>{
  await setLocale(locale);document.documentElement.setAttribute('data-theme',theme);
  show(<Storage search={{}} onSearch={vi.fn()} />);
  expect(await screen.findByText('Workspace recovery')).toBeVisible();
  expect(screen.getByText('Cached image')).toBeVisible();
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

it('selection sends only the chosen opaque records to the common cleanup opener',async()=>{
  const user=userEvent.setup(),open=vi.fn();show(<Storage search={{}} onSearch={vi.fn()} onCleanup={open} />);
  await user.click(await screen.findByRole('checkbox',{name:'Select Cached image'}));
  await user.click(screen.getByRole('button',{name:'Clean up…'}));
  expect(open).toHaveBeenCalledExactlyOnceWith(['image']);
});
