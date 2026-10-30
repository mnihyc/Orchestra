/** Pasted HTML uses the same inert source as native imports, retaining structure and explicit losses. */
import { describe, expect, it } from 'vitest';
import { readHtml } from '@/features/people/html';

describe('inert HTML conversion', () => {
  it('preserves a nested list and a table header while removing active content and unsafe links', () => {
    const result = readHtml('<ul><li>Outer<ul><li>Inner</li></ul></li></ul><table><tr><th>Name</th></tr><tr><td>Ada</td></tr></table><script>globalThis.convertedUnsafe = true</script><p><a href="javascript:alert(1)">Keep text</a></p>');
    expect(result.pieces[0]?.children[0]?.type).toBe('bullet');
    expect(result.pieces[1]?.props.header_row).toBe(true);
    expect(result.warnings).toEqual(['Unsafe embedded content was removed.', 'Unsafe links were left as text.']);
    expect('convertedUnsafe' in globalThis).toBe(false);
    expect(JSON.stringify(result.pieces)).not.toContain('javascript:');
  });
  it('keeps heading levels, character references and code as authored text', () => {
    const result = readHtml('<h6>Names &amp; &#x4e2d;</h6><pre>&lt;script&gt;plain&lt;/script&gt;</pre>');
    expect(result.pieces[0]?.props.level).toBe(6);
    expect(JSON.stringify(result.pieces[0]?.content)).toContain('Names & 中');
    expect(JSON.stringify(result.pieces[1]?.content)).toContain('<script>plain</script>');
  });

  it('reads what Orch writes as HTML: toggles, folding headings, callouts, columns, colours, code languages and to-dos', () => {
    const page = [
      '<details class="toggle"><summary>More</summary>\n<p>inside</p></details>',
      '<details class="fold" open><summary><h2>Folds</h2></summary>\n<p>under</p></details>',
      '<aside class="callout tone-warning"><p>Mind the gap</p>\n<p>second</p></aside>',
      '<pre class="wrap"><code class="language-rust">fn main() {}</code></pre>',
      '<figure class="diagram"><pre><code class="language-mermaid">graph TD\nA--&gt;B</code></pre><figcaption>Diagram, as its Mermaid source</figcaption></figure>',
      '<div class="columns"><div class="column" style="flex-grow:2"><p>left</p></div><div class="column"><p>right</p></div></div>',
      '<div class="page-break" role="separator"></div>',
      '<p class="t0 b2 align-center">Coloured <span class="t7">words</span></p>',
      '<ul class="todos"><li><input type="checkbox" disabled checked> done<ul><li>nested</li></ul></li><li><input type="checkbox" disabled> to do</li></ul>',
      '<table><colgroup><col style="width:30.00%"><col style="width:70.00%"></colgroup><thead><tr><th scope="col">A</th><th scope="col">B</th></tr></thead><tbody><tr><td class="t1 b3" colspan="2"><p>x</p><p>y</p></td></tr></tbody></table>',
    ].join('\n');
    const { pieces } = readHtml(page);
    expect(pieces.map((one) => one.type)).toEqual(['toggle', 'heading', 'callout', 'code', 'diagram', 'columns', 'page_break', 'paragraph', 'todo', 'todo', 'table']);
    expect(pieces[0]?.children[0]?.type).toBe('paragraph');
    expect(pieces[1]?.props).toMatchObject({ level: 2, toggle: true });
    expect(pieces[1]?.children).toHaveLength(1);
    expect(pieces[2]?.props.tone).toBe('warning');
    expect(pieces[2]?.children).toHaveLength(1);
    expect(pieces[3]?.props).toMatchObject({ language: 'rust', wrap: true });
    expect(pieces[4]?.props.source).toBe('graph TD\nA-->B');
    expect(pieces[5]?.children.map((column) => column.props.share)).toEqual([2, undefined]);
    expect(pieces[7]?.props).toMatchObject({ color: 0, background: 2, align: 'center' });
    expect(JSON.stringify(pieces[7]?.content)).toContain('"color":7');
    expect(pieces[8]?.props.checked).toBe(true);
    expect(pieces[8]?.children[0]?.type).toBe('bullet');
    expect(pieces[9]?.props.checked).toBe(false);
    expect(pieces[10]?.props).toMatchObject({ header_row: true, widths: [30, 70] });
    const cell = (pieces[10]?.content as { cells: { colspan?: number; color?: number; background?: number; content: unknown[] }[] }[])[1]?.cells[0];
    expect(cell).toMatchObject({ colspan: 2, color: 1, background: 3 });
    expect(cell?.content).toHaveLength(3);
  });
  it('makes a picture a block that names it, wherever it stands, and leaves what the picture is to whoever settles it', () => {
    const { pieces, images } = readHtml('<figure class="align-center"><img src="data:image/png;base64,iVBORw0KGgo=" alt="pic" style="width:300px"><figcaption>The pic</figcaption></figure><p>Before <img src="images/a.png" alt="rel"> after</p>');
    expect(pieces.map((one) => one.type)).toEqual(['image', 'paragraph', 'image', 'paragraph']);
    expect(pieces[0]?.props).toMatchObject({ width: 300, align: 'center' });
    expect(images.map((image) => image.src)).toEqual(['data:image/png;base64,iVBORw0KGgo=', 'images/a.png']);
    expect(pieces[0]?.props.file_id).toBe(images[0]?.id);
  });
  it('keeps a link that has no address of its own as a path for the caller, and never one a script could follow', () => {
    const { pieces } = readHtml('<p><a href="other.md#part">next</a> <a href="javascript:alert(1)">bad</a> <a href="data:text/html,x">data</a></p>');
    const runs = pieces[0]?.content as { text: string; href?: string }[];
    expect(runs.find((run) => run.text === 'next')?.href).toBe('other.md#part');
    expect(runs.find((run) => run.text === 'bad')?.href).toBeUndefined();
    expect(runs.find((run) => run.text === 'data')?.href).toBeUndefined();
  });
  it('reads XHTML', () => {
    const { pieces } = readHtml('<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>One<br/>two</p><hr/></body></html>');
    expect(pieces.map((one) => one.type)).toEqual(['paragraph', 'divider']);
  });
});
