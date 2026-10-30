/** The same page/section geometry serves every scope, including a topic opened from its Settings tab. */
import { type ReactNode, useContext, useId } from 'react';

import { Page, PageHeader, type PageHeaderProps, PageSectionContext } from '@/shell/Page';
import { SectionNav, type SectionNavProps } from '@/ui/SectionNav';

export interface SettingsFrameProps extends Pick<SectionNavProps, 'label' | 'current' | 'items'>, Pick<PageHeaderProps, 'adminAccess' | 'private' | 'opening'> {
  title: string;
  mark?: ReactNode;
  lead?: ReactNode;
  children: ReactNode;
}

export function SettingsFrame({ title, mark, lead, label, current, items, adminAccess, private: privatePage, opening, children }: SettingsFrameProps) {
  const headingId = useId();
  const parent = useContext(PageSectionContext);
  const namedByParent = parent !== null && parent.title === title;
  const context = { title, id: namedByParent ? parent.id : headingId, lead };
  return <Page>
    <PageHeader title={title} headingId={headingId} titleInContext={namedByParent} mark={mark} lead={lead} adminAccess={adminAccess} private={privatePage} opening={opening} />
    <div data-settings-frame className="@container">
      <div className="flex min-w-0 flex-col items-stretch gap-6 @min-[600px]:flex-row @min-[600px]:items-start">
        <SectionNav label={label} current={current} items={items} />
        <div data-settings-content className="flex min-w-0 flex-1 flex-col gap-6"><PageSectionContext value={context}>{children}</PageSectionContext></div>
      </div>
    </div>
  </Page>;
}
