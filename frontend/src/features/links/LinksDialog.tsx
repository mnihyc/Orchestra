/** The tabs of the Links section that one document shows: a checkbox for each, set by whoever manages the document. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentLinksQuery, LINKS_TABS, linksKey, setDocumentLinks } from '@/api/links';
import type { LinksTab } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function LinksDialog({ documentId, open, onOpenChange }: { documentId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const read = useQuery({ ...documentLinksQuery(documentId), enabled: open });
  // Nothing is chosen until a box is pressed; until then the boxes show what the document shows now.
  const [draft, setDraft] = useState<readonly LinksTab[] | null>(null);
  const links = read.data;
  const current = draft ?? links?.shown ?? [];
  const write = useMutation({
    mutationFn: (shown: LinksTab[] | null) => setDocumentLinks(documentId, shown),
    onSuccess: (next) => {
      cache.setQueryData([...linksKey(documentId), 'tabs'], next);
      setDraft(null);
      onOpenChange(false);
    },
  });
  const close = (next: boolean) => { if (!next) setDraft(null); onOpenChange(next); };
  const locked = links?.locked === true;
  const writable = links?.writable === true;
  return <Dialog open={open} onOpenChange={close} title={t('links.dialog.title')} size="sm"
    footer={links === undefined ? undefined : <div className="flex w-full min-w-0 flex-wrap items-center gap-2">
      <span className="min-w-0 flex-1 text-sm text-text-muted">{locked ? t('links.dialog.locked') : ''}</span>
      {links.own !== null && !locked && <Button variant="ghost" disabled={!writable || write.isPending} onClick={() => write.mutate(null)}>{t('links.dialog.useDefault')}</Button>}
      <Button variant="secondary" onClick={() => close(false)}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={write.isPending} disabled={!writable || draft === null} onClick={() => write.mutate([...current])}>{t('common.save')}</Button>
    </div>}>
    {read.isPending ? <SkeletonRows rows={4} /> : read.isError ? <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void read.refetch()}>{t('common.tryAgain')}</Button>} />
      : <div className="flex flex-col gap-3">
        {LINKS_TABS.map((tab) => <Checkbox key={tab} label={t(`links.tabs.${tab}`)} checked={current.includes(tab)} disabled={!writable}
          onCheckedChange={(on) => setDraft(LINKS_TABS.filter((each) => (each === tab ? on : current.includes(each))))} />)}
        {write.isError && <ErrorAlert error={write.error} />}
      </div>}
  </Dialog>;
}
