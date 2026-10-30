/** PDF pages, with a jump to a page, zoom and a search of the text.
 * The bytes come through the same authenticated file door as other viewers.
 * Only the chosen page has a canvas and text layer; pages and workers are
 * cleaned up on navigation/close. All PDF.js assets are hosted by Orch.
 */
import { ChevronLeft, ChevronRight, Minus, Plus, Search } from 'lucide-react';
import { getDocument, GlobalWorkerOptions, TextLayer, type PDFDocumentProxy, type PDFPageProxy, type RenderTask } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import 'pdfjs-dist/web/pdf_viewer.css';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';

import { useFileBytes } from '../content';
import { MEDIA_MOST_BYTES, type ViewerProps } from '../viewers';
import { LocalBinaryData } from './pdfAssets';
import { FileState, fileFailure } from '../FilePreview';
import { useMediaPreview } from '../mediaPolicy';

GlobalWorkerOptions.workerSrc = workerUrl;

/** Eight million RGBA pixels use 32 MiB, ample for a page on a dense display;
 * zoom changes its CSS size while larger canvases reduce their pixel density. */
const CANVAS_PIXELS = 8_000_000;
const ZOOM_STEP = 1.25;

export interface PdfMatch { page: number; count: number }

/** Searches one page at a time, keeping only page numbers/counts. A new search
 * or closing the view cancels the scan and releases its page resources. */
export async function findPdf(pdf: PDFDocumentProxy, query: string, signal: AbortSignal): Promise<PdfMatch[]> {
  const wanted = query.trim().toLocaleLowerCase();
  const found: PdfMatch[] = [];
  if (wanted === '') return found;
  for (let number = 1; number <= pdf.numPages; number++) {
    signal.throwIfAborted();
    const page = await pdf.getPage(number);
    try {
      const content = await page.getTextContent();
      signal.throwIfAborted();
      const text = content.items.map((item) => 'str' in item ? item.str : '').join(' ').toLocaleLowerCase();
      let count = 0;
      for (let at = text.indexOf(wanted); at >= 0; at = text.indexOf(wanted, at + wanted.length)) count++;
      if (count > 0) found.push({ page: number, count });
    } finally { page.cleanup(); }
  }
  return found;
}

function Page({ pdf, number, zoom, width, search }: { pdf: PDFDocumentProxy; number: number; zoom: number | 'fit'; width: number; search: string }) {
  const { t } = useTranslation();
  const canvas = useRef<HTMLCanvasElement>(null);
  const text = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<unknown>(null);
  useEffect(() => {
    const surface = canvas.current;
    const words = text.current;
    if (surface === null || words === null) return;
    const lifetime = new AbortController();
    let page: PDFPageProxy | null = null;
    let painting: RenderTask | null = null;
    let layer: TextLayer | null = null;
    void (async () => {
      try {
        page = await pdf.getPage(number);
        lifetime.signal.throwIfAborted();
        const base = page.getViewport({ scale: 1 });
        const scale = zoom === 'fit' ? Math.max(0.1, (width - 24) / base.width) : zoom;
        const viewport = page.getViewport({ scale });
        const density = Math.min(window.devicePixelRatio || 1, 2, Math.sqrt(CANVAS_PIXELS / (viewport.width * viewport.height)));
        surface.width = Math.max(1, Math.floor(viewport.width * density));
        surface.height = Math.max(1, Math.floor(viewport.height * density));
        surface.style.width = `${viewport.width}px`;
        surface.style.height = `${viewport.height}px`;
        words.replaceChildren();
        words.style.setProperty('--total-scale-factor', String(viewport.scale));
        const content = await page.getTextContent();
        lifetime.signal.throwIfAborted();
        painting = page.render({ canvas: surface, viewport, transform: [density, 0, 0, density, 0, 0] });
        layer = new TextLayer({ textContentSource: content, container: words, viewport });
        await Promise.all([painting.promise, layer.render()]);
        lifetime.signal.throwIfAborted();
        const wanted = search.trim().toLocaleLowerCase();
        for (let at = 0; wanted !== '' && at < layer.textDivs.length; at++) {
          if (layer.textContentItemsStr[at]?.toLocaleLowerCase().includes(wanted)) layer.textDivs[at]?.classList.add('bg-yellow-200/50');
        }
        setError(null);
      } catch (fault) {
        if (lifetime.signal.aborted) page?.cleanup();
        else setError(fault);
      }
    })();
    return () => {
      lifetime.abort();
      painting?.cancel();
      layer?.cancel();
      words.replaceChildren();
      surface.width = 0;
      surface.height = 0;
      page?.cleanup();
    };
  }, [pdf, number, zoom, width, search]);
  return (
    <div className="relative mx-auto w-fit shrink-0 bg-white shadow-sm">
      {error !== null && <ErrorState compact error={error} title={t('fileMedia.unavailable')} />}
      <canvas ref={canvas} aria-label={t('files.pdf.page', { page: number })} />
      <div ref={text} className="textLayer" />
    </div>
  );
}

function LoadedPdf({ pdf }: { pdf: PDFDocumentProxy }) {
  const { t } = useTranslation();
  const [number, setNumber] = useState(1);
  const [draft, setDraft] = useState('1');
  const [zoom, setZoom] = useState<number | 'fit'>('fit');
  const [width, setWidth] = useState(800);
  const [query, setQuery] = useState('');
  const [search, setSearch] = useState('');
  const [matches, setMatches] = useState<PdfMatch[]>([]);
  const [finding, setFinding] = useState(false);
  const [findError, setFindError] = useState<unknown>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const scan = useRef<AbortController | null>(null);
  useEffect(() => {
    const node = scroller.current;
    if (node === null) return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry !== undefined) setWidth(entry.contentRect.width);
    });
    observer.observe(node);
    return () => { observer.disconnect(); scan.current?.abort(); };
  }, []);
  const go = (next: number) => {
    if (!Number.isFinite(next)) return;
    const page = Math.max(1, Math.min(pdf.numPages, Math.trunc(next)));
    setNumber(page); setDraft(String(page));
    scroller.current?.scrollTo({ top: 0, left: 0 });
  };
  const find = () => {
    scan.current?.abort();
    const controller = new AbortController();
    scan.current = controller;
    setFinding(true); setFindError(null); setSearch(query);
    void findPdf(pdf, query, controller.signal).then((found) => {
      if (controller.signal.aborted) return;
      setMatches(found);
      if (found[0] !== undefined) go(found[0].page);
    }).catch((error: unknown) => { if (!controller.signal.aborted) setFindError(error); })
      .finally(() => { if (!controller.signal.aborted) setFinding(false); });
  };
  const nextMatch = (direction: 1 | -1) => {
    const ordered = direction === 1 ? matches : [...matches].reverse();
    const target = ordered.find((match) => direction === 1 ? match.page > number : match.page < number) ?? ordered[0];
    if (target !== undefined) go(target.page);
  };
  const step = (factor: number) => setZoom((current) => Math.max(0.25, Math.min(4, (current === 'fit' ? 1 : current) * factor)));
  return (
    <div className="flex h-full min-h-0 flex-col" data-viewer="pdf">
      <div className="flex flex-wrap items-center gap-1 border-b border-border p-2">
        <IconButton size="sm" label={t('files.pdf.previous')} icon={<ChevronLeft aria-hidden />} disabled={number === 1} onClick={() => go(number - 1)} />
        <form className="flex items-center gap-1" onSubmit={(event) => { event.preventDefault(); go(Number(draft)); }}>
          <Input type="number" min={1} max={pdf.numPages} value={draft} label={t('files.pdf.pageNumber')} hideLabel className="w-20" onChange={(event) => setDraft(event.target.value)} />
          <Button size="sm" variant="ghost" type="submit">{t('files.pdf.go')}</Button>
        </form>
        <span className="text-xs tabular-nums" aria-live="polite">{t('files.pdf.pageOf', { page: number, total: pdf.numPages })}</span>
        <IconButton size="sm" label={t('files.pdf.next')} icon={<ChevronRight aria-hidden />} disabled={number === pdf.numPages} onClick={() => go(number + 1)} />
        <Button size="sm" variant={zoom === 'fit' ? 'secondary' : 'ghost'} onClick={() => setZoom('fit')}>{t('files.image.fit')}</Button>
        <IconButton size="sm" label={t('files.image.zoomOut')} icon={<Minus aria-hidden />} onClick={() => step(1 / ZOOM_STEP)} />
        <IconButton size="sm" label={t('files.image.zoomIn')} icon={<Plus aria-hidden />} onClick={() => step(ZOOM_STEP)} />
      </div>
      <form className="flex flex-wrap items-center gap-1 border-b border-border p-2" onSubmit={(event) => { event.preventDefault(); find(); }}>
        <Input type="search" label={t('files.pdf.search')} hideLabel value={query} onChange={(event) => setQuery(event.target.value)} className="min-w-24 flex-1" />
        <Button size="sm" type="submit" variant="ghost" pending={finding} icon={<Search aria-hidden />}>{t('files.pdf.find')}</Button>
        {search !== '' && !finding && <span role="status" className="text-xs">{t('files.pdf.matches', { count: matches.reduce((sum, match) => sum + match.count, 0) })}</span>}
        <IconButton size="sm" label={t('files.pdf.previousMatch')} icon={<ChevronLeft aria-hidden />} disabled={matches.length === 0 || finding} onClick={() => nextMatch(-1)} />
        <IconButton size="sm" label={t('files.pdf.nextMatch')} icon={<ChevronRight aria-hidden />} disabled={matches.length === 0 || finding} onClick={() => nextMatch(1)} />
      </form>
      {findError !== null && <ErrorState compact error={findError} onRetry={find} />}
      <div ref={scroller} className="flex min-h-0 flex-1 overflow-auto bg-bg-muted p-3">
        <Page pdf={pdf} number={number} zoom={zoom} width={width} search={search} />
      </div>
    </div>
  );
}

/** Also used by a document file's authorized preview. */
export function PdfPages({ bytes }: { bytes: Uint8Array }) {
  const { t } = useTranslation();
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{ bytes: Uint8Array; attempt: number; pdf: PDFDocumentProxy | null; error: unknown } | null>(null);
  useEffect(() => {
    // PDF.js takes ownership of its array; the file cache retains its own bytes.
    const task = getDocument({ data: bytes.slice(), useWorkerFetch: false,
      BinaryDataFactory: LocalBinaryData, canvasMaxAreaInBytes: CANVAS_PIXELS * 4 });
    let live = true;
    void task.promise.then((pdf) => { if (live) setState({ bytes, attempt, pdf, error: null }); })
      .catch((error: unknown) => { if (live) setState({ bytes, attempt, pdf: null, error }); });
    return () => { live = false; void task.destroy().catch(() => undefined); };
  }, [bytes, attempt]);
  const current = state?.bytes === bytes && state.attempt === attempt ? state : null;
  if (current === null) return <SkeletonRows rows={6} />;
  if (current.error !== null || current.pdf === null) return <ErrorState compact title={t('fileMedia.unavailable')} error={current.error} onRetry={() => setAttempt((value) => value + 1)} />;
  return <LoadedPdf pdf={current.pdf} />;
}

export default function PdfViewer({ door, path, head }: ViewerProps) {
  const preview = useMediaPreview(door, path, head);
  const tooBig = head.stat.size > (preview.data?.viewer_bytes ?? MEDIA_MOST_BYTES);
  const read = useFileBytes(door, path, tooBig || preview.data === undefined ? undefined : head.brought);
  if (tooBig) return <FileState state="too_large" />;
  if (preview.isError) return <FileState state={fileFailure(preview.error)} />;
  if (head.brought.state === 'unavailable') return <FileState state={head.brought.reason === 'not_found' ? 'gone' : 'unavailable'} />;
  if (read.error !== null) return <FileState state={fileFailure(read.error)} />;
  if (read.bytes === null) return <SkeletonRows rows={6} />;
  return <PdfPages bytes={read.bytes} />;
}
