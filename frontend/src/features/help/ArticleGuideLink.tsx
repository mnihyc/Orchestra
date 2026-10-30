/** A guide is named by its title and occupies one entry for its article and anchor on the active screen. */
import { BookOpen } from 'lucide-react';
import type { ReactNode } from 'react';

import type { SettingsTarget } from '@/api/settings';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { useHelpState } from '@/features/help/state';
import { Button } from '@/ui/Button';

export function ArticleGuideLink({ article, heading = null, title, icon = <BookOpen aria-hidden />, target }: { article: string; heading?: string | null; title: string; icon?: ReactNode; target?: SettingsTarget }) {
  const open = useHelpOpener();
  const { ref, hidden } = useGuideEntry(article, heading, 'guide');
  return <span ref={ref} className="contents">{!hidden && <Button action="help" size="sm" variant="ghost" icon={icon}
    data-help-guide={article} data-help-article={article} data-help-heading={heading ?? ''} aria-label={title}
    onClick={() => {
      if (heading === null) open(article); else open(article, heading);
      useHelpState.setState({ guideContext: target === undefined ? null : { article, target } });
    }}>{title}</Button>}</span>;
}
