/** Real Table/selection primitives with wrapped data, plus unmodified controls outside the table. */
import { flushSync } from 'react-dom';
import { createRoot, type Root } from 'react-dom/client';
import { useState } from 'react';
import { ArrowRight, Ellipsis } from 'lucide-react';
import { i18n } from '@/i18n';
import { AllBox, RowBox } from '@/features/resources/acts';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { IconButton } from '@/ui/IconButton';
import { Menu } from '@/ui/Menu';
import { StatusDot } from '@/ui/StatusDot';
import { Table, type Column } from '@/ui/Table';
import { TooltipProvider } from '@/ui/Tooltip';
import { detailEnvironment } from './resource-detail-fixtures';
import type { TableCase, TableHarness } from './table-first-line-types';

interface Row { id: string; name: string }
let root: Root | undefined;
let host: HTMLDivElement | undefined;

function Example({ kind, chinese }: { kind: TableCase; chinese: boolean }) {
  const [selected, select] = useState(false), [last, act] = useState('');
  const name = chinese ? '完整的多行名称，包括末尾的文字和不同格式' : 'A complete wrapped name with its final words and mixed formatting';
  const description = chinese ? '明确的小号多行文字，保留完整的内容和自然换行。' : 'Explicit small multiline text retains all its words and natural wrapping.';
  const row = { id: 'one', name };
  const secondary = <div data-secondary><p className="max-w-[24ch] text-sm">{description} {description}</p></div>;
  const columns: Column<Row>[] = [
    { id: 'select', grow: false, header: <AllBox checked={selected} onChange={select} />, cell: () => <RowBox name={name} checked={selected} onChange={select} /> },
    { id: 'name', header: chinese ? '名称' : 'Name', grow: true, cell: () => kind === 'resource'
      ? <div className="flex min-w-0 flex-col gap-1"><ResourceChip subject={{ kind: 'environment', environment: detailEnvironment }} card={false} hint={false} />{secondary}</div>
      : kind === 'small' ? <><p className="text-sm whitespace-normal">{name}</p>{secondary}</>
        : <>{chinese ? '完整的 ' : 'Complete '}<strong>{chinese ? '多行' : 'mixed'}</strong>{chinese ? ' 名称包括末尾的文字和不同格式' : ' inline name with every final word and natural wrapping'}{secondary}</> },
    { id: 'status', header: chinese ? '状态' : 'Status', from: 'sm', cell: () => <StatusDot tone="success" label={chinese ? '在线' : 'Online'} showLabel /> },
    { id: 'primary', header: chinese ? '打开' : 'Open', hideHeader: true, align: 'end', cell: () => <Button size="sm" icon={<ArrowRight aria-hidden />} onClick={() => act('opened')}>{chinese ? '打开' : 'Open'}</Button> },
    { id: 'acts', header: chinese ? '操作' : 'Actions', hideHeader: true, align: 'end', cell: () => <Menu label={chinese ? '这一行的操作' : 'Actions for this row'} trigger={<IconButton size="sm" label={chinese ? '这一行的操作' : 'Actions for this row'} icon={<Ellipsis aria-hidden />} />} entries={[{ id: 'inspect', label: chinese ? '检查' : 'Inspect', onSelect: () => act('inspected') }]} /> },
  ];
  return <TooltipProvider><div className="flex min-w-0 flex-col gap-4 bg-bg p-4 text-text">
    <Table boundedIdentity={kind !== 'mixed'} label="First-line controls" columns={columns} rows={[row]} rowKey={(value) => value.id} />
    <div data-outside-form className="w-48 max-w-full"><Checkbox label={`${chinese ? '表单' : 'Form'}: ${name}`} description={description} checked={selected} onCheckedChange={select} /></div>
    <div data-outside-compact className="w-48 max-w-full"><Checkbox presentation="compact" label={`${chinese ? '紧凑' : 'Compact'}: ${name}`} checked={selected} onCheckedChange={select} /></div>
    <div data-outside-chip><ResourceChip subject={{ kind: 'environment', environment: detailEnvironment }} card={false} hint={false} /></div>
    <div data-outside-status><StatusDot tone="success" label={chinese ? '在线' : 'Online'} showLabel /></div>
    <output aria-label="selection result">{selected ? 'selected' : 'not selected'}</output><output aria-label="action result">{last}</output>
  </div></TooltipProvider>;
}

export const mountTableCase: TableHarness['mountTableCase'] = async (kind, locale, theme) => {
  flushSync(() => root?.unmount()); host?.remove();
  await i18n.changeLanguage(locale); document.documentElement.lang = locale; document.documentElement.dataset.theme = theme;
  host = document.createElement('div'); host.dataset.tableExample = 'true';
  Object.assign(host.style, { position: 'fixed', inset: '0', zIndex: '100', overflow: 'auto', background: 'var(--color-bg)' });
  document.body.append(host); root = createRoot(host);
  flushSync(() => root?.render(<Example kind={kind} chinese={locale === 'zh-CN'} />));
};
