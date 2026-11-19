/** A document as one file: Markdown with its losses, a PDF typeset locally, HTML or Typst source, and choices for attachments and discussions.
 *
 * One dialog for whoever may take the document away. The server writes the text and the images always; the
 * attachments and the comments are two ticks, empty each time the dialog opens, and an export with files is an
 * archive. What Markdown cannot keep is told before the file is made, from the blocks of the document, and the server's
 * own list follows once it is saved. PDF uses the server's Typst and authorized
 * images in a local compiler. Print is available when typesetting cannot load.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate, useRouterState } from '@tanstack/react-router';
import { Printer } from 'lucide-react';
import { type ReactElement, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { exportDocumentFile, exportedBlocksQuery, exportedDocumentQuery } from '@/api/exports';
import { compilerAvailable, printPage, typesetPdf } from '@/features/documents/pdf';
import type { DocumentExport } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { useDocumentState } from '@/realtime/stores/document';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { InlineAlert } from '@/ui/InlineAlert';
import { RadioGroup } from '@/ui/RadioGroup';
import { SkeletonRows } from '@/ui/Skeleton';

import { Group } from './Group';
import { markdownLosses } from './losses';
import { saveFile } from './save';
import type { FileDialogProps } from './ThreadFile';

/** The formats the server writes a document in. */
type Written = Extract<DocumentExport, 'markdown' | 'html' | 'typst'>;
/** What the dialog offers: those, and the PDF the browser makes. */
type Choice = Written | 'pdf';

/** How long the dialog has to be gone before the page is printed: a dialog that is still there would be printed with it. */
const GONE_MS = 100;

export function DocumentFile({ target, open, onClose }: FileDialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const locale = useAppearance((state) => state.locale);
  const read = useQuery(exportedDocumentQuery(target.id));
  const [format, setFormat] = useState<Choice>('markdown');
  const [attachments, setAttachments] = useState(false);
  const [comments, setComments] = useState(false);
  const [losses, setLosses] = useState<string[] | null>(null);
  const here = useRouterState({ select: (state) => state.location.pathname === `/doc/${target.id}` });

  const document = read.data;
  const files = document?.files.filter((file) => !file.media_type.startsWith('image/')) ?? [];
  const bytes = files.reduce((sum, file) => sum + file.size, 0);
  const allowed = document === undefined ? null : holds(document.access, 'doc.export');

  // What Markdown could not say of the document is told before the file is made: from the blocks the page holds where the document is open
  // in it, from the blocks read once where it is not.
  const held = useDocumentState(target.id);
  const blocksRead = useQuery({ ...exportedBlocksQuery(target.id), enabled: format === 'markdown' && !held.loaded });
  const cannot = useMemo(() => {
    const blocks = held.loaded ? [...held.blocks.values()] : (blocksRead.data ?? null);
    return blocks === null ? [] : markdownLosses(blocks);
  }, [held.loaded, held.blocks, blocksRead.data]);

  // A PDF is the browser's own where the page holds no compiler; the page that is printed is the document's.
  const byPrint = format === 'pdf' && !compilerAvailable();

  const make = useMutation({
    mutationFn: async () => {
      const file = await exportDocumentFile(target.id, { format: format === 'pdf' ? 'typst' : format, attachments: format !== 'pdf' && attachments ? 'include' : 'omit', comments: comments ? 'include' : 'omit' });
      if (format !== 'pdf') return file;
      const pdf = await typesetPdf(file.blob);
      return { ...file, blob: pdf.blob, name: file.name.replace(/\.zip$/i, '') + '.pdf', losses: [...file.losses, ...pdf.losses] };
    },
    onSuccess: (file) => {
      saveFile(file.blob, file.name);
      if (file.losses.length === 0) onClose();
      else setLosses(file.losses);
    },
  });

  const print = () => {
    onClose();
    setTimeout(printPage, GONE_MS);
  };
  const openToPrint = () => {
    onClose();
    void navigate({ to: '/doc/$documentId', params: { documentId: target.id } });
  };

  let body: ReactElement;
  if (read.isError) {
    body = <ErrorState compact error={read.error} retrying={read.isRefetching} onRetry={() => void read.refetch()} />;
  } else if (document === undefined) {
    body = <SkeletonRows rows={4} />;
  } else {
    body = (
      <div className="flex flex-col gap-4">
        {allowed === false && <InlineAlert tone="warning">{t('export.menu.notAllowed', { name: document.owner.display_name })}</InlineAlert>}
        <Group label={t('export.document.as')} lead="box" help={<HelpHint term={t('export.document.as')} what={t(format === 'pdf' ? byPrint ? 'choiceCatalog.documentFormat.browser_pdf.description' : 'choiceCatalog.documentFormat.pdf.description' : format === 'html' ? 'choiceCatalog.documentFormat.html.description' : format === 'typst' ? 'choiceCatalog.documentFormat.typst.description' : 'choiceCatalog.documentFormat.markdown.description')} />}>
          <RadioGroup<Choice>
            label={t('export.document.as')}
            variant="list"
            value={format}
            onValueChange={setFormat}
            options={[
              { value: 'markdown', ...choiceOption(t, 'documentFormat.markdown') },
              { value: 'pdf', ...choiceOption(t, byPrint ? 'documentFormat.browser_pdf' : 'documentFormat.pdf') },
              { value: 'html', ...choiceOption(t, 'documentFormat.html') },
              { value: 'typst', ...choiceOption(t, 'documentFormat.typst') },
            ]}
          />
        </Group>
        {format === 'markdown' && cannot.length > 0 && (
          <InlineAlert tone="info" title={t('export.document.willLose')}>
            <ul className="list-disc pl-4">
              {cannot.map((loss) => (
                <li key={loss.kind}>{t(`export.document.loses.${loss.kind}`, { count: loss.count })}</li>
              ))}
            </ul>
          </InlineAlert>
        )}
        {byPrint && !here && <InlineAlert tone="info">{t('export.document.pdfElsewhere')}</InlineAlert>}
        {!byPrint && (
          <Group label={t('export.document.include')} lead="text" help={<HelpHint term={t('export.document.include')} what={<ul className="flex flex-col gap-2"><li>{format === 'pdf' ? t('export.document.attachmentsPdf') : files.length === 0 ? t('export.document.noAttachments') : t('export.document.attachmentsAbout', { count: files.length, size: formatBytes(bytes, locale) })}</li><li>{t('export.document.commentsAbout', { count: document.open_discussions })}</li></ul>} />}>
            <div className="flex flex-col gap-2">
              <Checkbox label={t('export.document.textAndImages')} hint={t('export.document.always')} checked disabled onCheckedChange={() => undefined} />
              <Checkbox
                label={t('export.document.attachments')}
                hint={format === 'pdf' ? t('export.document.attachmentsPdf') : files.length === 0 ? t('export.document.noAttachments') : t('export.document.attachmentsAbout', { count: files.length, size: formatBytes(bytes, locale) })}
                checked={attachments}
                disabled={files.length === 0 || format === 'pdf'}
                onCheckedChange={setAttachments}
              />
              <Checkbox
                label={t('export.document.comments')}
                hint={t('export.document.commentsAbout', { count: document.open_discussions })}
                checked={comments}
                onCheckedChange={setComments}
              />
            </div>
          </Group>
        )}
        {losses !== null && (
          <InlineAlert tone="info">
            {t('export.document.saved')} {t('export.document.cannotSay', { what: losses.join(', ') })}
          </InlineAlert>
        )}
        {make.isError && <ErrorAlert error={make.error} />}
        {make.isError && format === 'pdf' && <InlineAlert tone="info">{t('export.document.pdfFallback')}</InlineAlert>}
      </div>
    );
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('export.document.title', { name: target.name })}
      dismissible={!make.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={make.isPending} onClick={onClose}>
            {losses === null ? t('common.cancel') : t('common.close')}
          </Button>
          {make.isError && format === 'pdf' && !byPrint && (
            here ? <Button variant="secondary" blocked={allowed !== true} icon={<Printer aria-hidden />} onClick={print}>{t('export.document.print')}</Button>
              : <Button variant="secondary" action="openDocument" onClick={openToPrint}>{t('export.document.openToPrint')}</Button>
          )}
          {losses === null &&
            (byPrint ? (
              here ? (
                <Button variant="primary" blocked={allowed !== true} icon={<Printer aria-hidden />} onClick={print}>
                  {t('export.document.print')}
                </Button>
              ) : (
                <Button action="openDocument" variant="primary" onClick={openToPrint}>
                  {t('export.document.openToPrint')}
                </Button>
              )
            ) : (
              <Button variant="primary" pending={make.isPending} blocked={allowed !== true} onClick={() => allowed === true && make.mutate()}>
                {t('export.document.do')}
              </Button>
            ))}
        </>
      }
    >
      {body}
    </Dialog>
  );
}
