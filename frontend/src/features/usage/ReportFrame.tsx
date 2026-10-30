/** One report frame for scope controls, figures, breakdowns and records. */
import type { ReactNode } from 'react';
import { Page, PageHeader, PlaceMark, boxClasses } from '@/shell/Page';
import { cx } from '@/ui/cx';
import { FilterBar } from '@/ui/FilterBar';
import { Tabs } from '@/ui/Tabs';

export function ReportFrame({ heading, title, icon, actions, filters, children }: {
  heading?: ReactNode;
  title: string;
  icon: ReactNode;
  actions?: ReactNode;
  filters: ReactNode;
  children: ReactNode;
}) {
  return <Page>
    {heading}
    <PageHeader mark={<PlaceMark>{icon}</PlaceMark>} title={title} actions={actions} />
    <FilterBar>{filters}</FilterBar>
    {children}
  </Page>;
}

/** A figure's size, label and spacing stay the same across reports. */
export function ReportFigure({ label, value, className, children }: { label: string; value: ReactNode; className?: string; children?: ReactNode }) {
  return <li className={cx(boxClasses, 'flex min-w-0 flex-col gap-1 px-4 py-3', className)}>
    <span className="text-sm font-medium text-text-muted">{label}</span>
    <span className="text-2xl font-semibold text-text tabular-nums">{value}</span>
    {children}
  </li>;
}

/** The same breakdown navigation and panel spacing in every report. */
export function ReportBreakdowns<Value extends string>({ label, value, onValueChange, tabs, className }: {
  label: string;
  value: Value;
  onValueChange: (value: Value) => void;
  tabs: { value: Value; label: string; menuLabel?: string; panel: ReactNode }[];
  className?: string;
}) {
  return <Tabs label={label} value={value} onValueChange={onValueChange} className={className} panelClassName="pt-3"
    tabs={tabs.map((tab) => ({ ...tab, panel: <div className="flex min-w-0 flex-col gap-3">{tab.panel}</div> }))} />;
}

/** The records heading and its optional scope statement belong to the shared report frame. */
export function ReportRecords({ id, title, about, actions, className, children }: {
  id: string;
  title: string;
  about?: ReactNode;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return <section aria-labelledby={id} className={className}>
    <div className="mb-2 flex items-center justify-between gap-2">
      <div className="flex min-w-0 flex-col gap-1"><h2 id={id} className="text-md font-semibold text-text">{title}</h2>{about}</div>
      {actions}
    </div>
    {children}
  </section>;
}
