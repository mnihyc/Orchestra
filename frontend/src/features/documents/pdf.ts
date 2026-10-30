/** A PDF typeset in the browser: the Typst compiler and the fonts a document needs, fetched from Orch itself when somebody first exports, and the diagrams the page drew put into the source the server wrote.
 *
 * The compiler, its worker and its body/math/code/CJK fonts are local build assets,
 * loaded on the first export. A project belongs to one export and is destroyed
 * afterwards, so simultaneous exports share no private source or compiler state.
 */
import { assetBytes } from '@/api/client';
import { i18n } from '@/i18n';

import { readTypstBundle } from './typstBundle';

/** Whether the Typst compiler and its fonts are part of this build. */
export function compilerAvailable(): boolean {
  return typeof WebAssembly !== 'undefined' && typeof Worker !== 'undefined' && typeof DecompressionStream !== 'undefined';
}

/** The compiler embeds its default body, math and monospace fonts. These two
 * local TTF faces add Chinese and its punctuation in regular and bold text. */
async function fontUrls(): Promise<string[]> {
  const [regular, bold] = await Promise.all([
    import('@electron-fonts/noto-sans-sc/fonts/NotoSansSC-Regular.ttf?url'),
    import('@electron-fonts/noto-sans-sc/fonts/NotoSansSC-Bold.ttf?url'),
  ]);
  return [regular.default, bold.default];
}

/** Replaces only the server's explicit diagram markers. Invalid Mermaid stays
 * as the server's labelled source and is named in the export's losses. */
export async function drawDiagrams(files: Map<string, Uint8Array>): Promise<string[]> {
  const sourceBytes = files.get('/main.typ');
  if (sourceBytes === undefined) throw new Error(i18n.t('documents.pdf.invalidBundle'));
  let source = new TextDecoder('utf-8', { fatal: true }).decode(sourceBytes);
  const pattern = /^(\s*)\/\/ orch:diagram ([\da-f-]+)\n\s*#figure\(raw\(block: true, lang: "mermaid", ("(?:\\.|[^"\\])*")\), caption: \[Diagram, as its Mermaid source\]\)/gm;
  const diagrams = [...source.matchAll(pattern)];
  const losses: string[] = [];
  if (diagrams.length > 0) {
    const { default: mermaid } = await import('mermaid');
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme: 'default', fontFamily: 'Noto Sans SC', flowchart: { htmlLabels: false } });
    for (const match of diagrams) {
      const [, indent = '', id = '', encoded = ''] = match;
      try {
        const diagram: unknown = JSON.parse(encoded);
        if (typeof diagram !== 'string') throw new Error(i18n.t('documents.pdf.invalidBundle'));
        const { svg } = await mermaid.render(`orch-export-${id}`, diagram);
        const path = `orch-diagrams/${id}.svg`;
        files.set(`/${path}`, new TextEncoder().encode(svg));
        source = source.replace(match[0], `${indent}#figure(image("${path}", width: 100%))`);
      } catch {
        losses.push(i18n.t('documents.pdf.diagramKept'));
      }
    }
  }
  // Font choices precede the server's source; its size and other presentation
  // remain authoritative. Raw text keeps the bundled monospace face.
  source = '#set text(font: ("Libertinus Serif", "Noto Sans SC"))\n#show raw: set text(font: ("DejaVu Sans Mono", "Noto Sans SC"))\n' + source;
  files.set('/main.typ', new TextEncoder().encode(source));
  return losses;
}

/** Typesets the authenticated server export, including every image it names,
 * entirely in a local worker. Compiler diagnostics become an actionable error. */
export async function typesetPdf(bundle: Blob): Promise<{ blob: Blob; losses: string[] }> {
  const files = await readTypstBundle(bundle);
  const losses = await drawDiagrams(files);
  const { TypstProject } = await import('@vedivad/typst-web-service');
  const project = await TypstProject.create({ entry: '/main.typ' });
  try {
    for (const url of await fontUrls()) {
      await project.addFont(await assetBytes(url));
    }
    await project.setMany(Object.fromEntries(files));
    const compiled = await project.compile();
    const errors = compiled.diagnostics.filter((diagnostic) => diagnostic.severity === 'error');
    if (errors.length > 0) throw new Error(errors.map((diagnostic) => diagnostic.message).join('\n'));
    const bytes = await project.exportPdf();
    if (bytes === undefined || bytes.length === 0) throw new Error(i18n.t('documents.pdf.failed'));
    return { blob: new Blob([bytes.slice()], { type: 'application/pdf' }), losses };
  } finally {
    project.destroy();
  }
}

/** Opens the browser's print dialog for the page, laid out for paper. */
export function printPage(): void {
  window.print();
}
