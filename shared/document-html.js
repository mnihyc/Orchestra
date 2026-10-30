/** Inert HTML-to-document conversion shared verbatim by the browser and the native import producer. */

// Limit recursive conversion to a depth that fits both the browser and the native runtime's stack.
const HTML_NESTING_LIMIT = 128;
const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr']);
const UNSAFE = new Set(['script', 'style', 'iframe', 'object', 'embed', 'link', 'meta', 'svg', 'noscript', 'template']);
const BLOCK = new Set(['p', 'div', 'section', 'article', 'main', 'header', 'footer', 'nav', 'aside', 'figure', 'details', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'blockquote', 'pre', 'table', 'hr', 'video', 'audio']);
const MARK = { strong: 'bold', b: 'bold', em: 'italic', i: 'italic', u: 'underline', ins: 'underline', s: 'strike', strike: 'strike', del: 'strike', code: 'code', kbd: 'code', samp: 'code', tt: 'code' };
const ENTITIES = {
  amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', copy: '©', reg: '®', trade: '™', ndash: '–', mdash: '—', hellip: '…', lsquo: '‘', rsquo: '’', ldquo: '“', rdquo: '”', bull: '•', middot: '·',
  laquo: '«', raquo: '»', euro: '€', pound: '£', yen: '¥', cent: '¢', sect: '\u00a7', para: '¶', deg: '°', plusmn: '±', times: '×', divide: '÷', larr: '←', rarr: '→', uarr: '↑', darr: '↓', harr: '↔', check: '✓', ne: '≠', le: '≤', ge: '≥', infin: '∞', shy: '',
};
// The twelve colours of the palette, in the order of their indexes (the classes `t0`…`t11` and `b0`…`b11` of Orch's own export).
const PALETTE = 12;
const TONES = new Set(['info', 'success', 'warning', 'danger']);
const ALIGNS = new Set(['center', 'right', 'justify']);
// The most columns a columns block holds, and the fewest.
const COLUMNS_MIN = 2;
const COLUMNS_MAX = 6;
// What a picture in a line of text, or in a cell, stands for where it cannot be a block.
const IMAGE_MARK = 'img';

function readHtml(source) {
  const losses = new Set();
  const images = [];
  const text = String(source);
  const folded = text.toLowerCase();
  const root = { tag: 'body', attrs: Object.create(null), children: [] };
  const stack = [root];
  const decode = (value) => value.replace(/&(#x[\da-f]+|#\d+|[a-z][a-z\d]+);/gi, (whole, entity) => {
    if (entity[0] !== '#') {
      if (Object.prototype.hasOwnProperty.call(ENTITIES, entity)) return ENTITIES[entity];
      losses.add('An unsupported character reference was kept as text.'); return whole;
    }
    const hex = entity[1].toLowerCase() === 'x';
    const code = Number.parseInt(entity.slice(hex ? 2 : 1), hex ? 16 : 10);
    return code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff) ? String.fromCodePoint(code) : '\ufffd';
  });
  const addText = (value) => { if (value !== '') stack[stack.length - 1].children.push({ text: decode(value) }); };
  let at = 0;
  while (at < text.length) {
    const start = text.indexOf('<', at);
    if (start < 0) { addText(text.slice(at)); break; }
    addText(text.slice(at, start));
    if (text.startsWith('<!--', start)) { const end = text.indexOf('-->', start + 4); at = end < 0 ? text.length : end + 3; continue; }
    if (text.startsWith('<![CDATA[', start)) {
      const end = text.indexOf(']]>', start + 9);
      const stop = end < 0 ? text.length : end;
      const raw = text.slice(start + 9, stop);
      if (raw !== '') stack[stack.length - 1].children.push({ text: raw });
      at = end < 0 ? text.length : end + 3; continue;
    }
    let end = start + 1; let quote = '';
    for (; end < text.length; end += 1) {
      const c = text[end];
      if (quote !== '') { if (c === quote) quote = ''; }
      else if (c === '"' || c === "'") quote = c;
      else if (c === '>') break;
    }
    if (end >= text.length) { addText(text.slice(start)); losses.add('Incomplete HTML was kept as text.'); break; }
    const token = text.slice(start + 1, end);
    at = end + 1;
    if (/^\s*[!?]/.test(token)) continue;
    const close = /^\s*\/\s*([a-z][\w:-]*)/i.exec(token);
    if (close !== null) {
      const tag = close[1].toLowerCase();
      for (let index = stack.length - 1; index > 0; index -= 1) {
        if (stack[index].tag === tag) { stack.length = index; break; }
      }
      continue;
    }
    const opening = /^\s*([a-z][\w:-]*)/i.exec(token);
    if (opening === null) { addText(text.slice(start, end + 1)); continue; }
    const tag = opening[1].toLowerCase().replace(/^xhtml:/, '');
    if (UNSAFE.has(tag)) {
      losses.add('Unsafe embedded content was removed.');
      if (!VOID.has(tag)) {
        const terminator = new RegExp(`</${tag}\\s*>`, 'gi');
        terminator.lastIndex = at; const found = terminator.exec(folded);
        at = found === null ? text.length : found.index + found[0].length;
      }
      continue;
    }
    // Common omitted closing tags are repaired without evaluating a browser document.
    if (tag === 'li' && stack[stack.length - 1].tag === 'li') stack.pop();
    if (['td', 'th'].includes(tag) && ['td', 'th'].includes(stack[stack.length - 1].tag)) stack.pop();
    if (tag === 'tr') while (['td', 'th', 'tr'].includes(stack[stack.length - 1].tag)) stack.pop();
    if (tag === 'p' && stack[stack.length - 1].tag === 'p') stack.pop();
    const attrs = Object.create(null);
    const pattern = /([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g;
    const tail = token.slice(opening[0].length);
    for (const attribute of tail.matchAll(pattern)) attrs[attribute[1].toLowerCase()] = decode(attribute[2] ?? attribute[3] ?? attribute[4] ?? '');
    const node = { tag, attrs, children: [] };
    stack[stack.length - 1].children.push(node);
    if (!VOID.has(tag) && !/\/\s*$/.test(token)) {
      if (stack.length >= HTML_NESTING_LIMIT) throw new Error('HTML nesting is too deep.');
      stack.push(node);
    }
  }

  // ---------------------------------------------------------------- what an element says
  const classesOf = (node) => (node.attrs === undefined || node.attrs.class === undefined ? [] : node.attrs.class.split(/\s+/).filter((one) => one !== ''));
  const hasClass = (node, name) => classesOf(node).includes(name);
  /** The colour and the background that the classes `tN` and `bN` of Orch's export say. */
  const tintsOf = (node) => {
    const out = {};
    for (const name of classesOf(node)) {
      const match = /^([tb])(\d{1,2})$/.exec(name);
      if (match === null) continue;
      const tint = Number(match[2]);
      if (tint >= PALETTE) continue;
      out[match[1] === 't' ? 'color' : 'background'] = tint;
    }
    return out;
  };
  /** What the whole block looks like: its colours, and how it is aligned. */
  const lookOf = (node) => {
    const out = tintsOf(node);
    for (const name of classesOf(node)) {
      const match = /^align-(center|right|justify)$/.exec(name);
      if (match !== null && ALIGNS.has(match[1])) out.align = match[1];
    }
    const style = node.attrs === undefined ? undefined : node.attrs.style;
    const aligned = style === undefined ? null : /text-align\s*:\s*(center|right|justify)/i.exec(style);
    if (aligned !== null && out.align === undefined) out.align = aligned[1].toLowerCase();
    return out;
  };
  const languageOf = (...nodes) => {
    for (const node of nodes) {
      if (node === undefined) continue;
      for (const name of classesOf(node)) {
        const match = /^(?:language|lang)-([\w#+.-]+)$/.exec(name);
        if (match !== null) return match[1];
      }
      const named = node.attrs === undefined ? undefined : node.attrs['data-language'] ?? node.attrs['data-lang'];
      if (named !== undefined && named !== '') return named;
    }
    return '';
  };

  // ---------------------------------------------------------------- inside a line
  const allowedLink = (href) => /^(?:https?:\/\/|mailto:)/i.test(href) || href.startsWith('#') || (href.startsWith('/') && !href.startsWith('//'));
  /** A link that goes nowhere a script could: an address with no scheme at all is a path, kept for the caller to settle. */
  const keptLink = (href) => allowedLink(href) || (!/^[a-z][a-z\d+.-]*:/i.test(href) && !href.startsWith('//') && !href.startsWith('\\') && href !== '' && !/[\u0000-\u001f]/.test(href));
  const imageOf = (node) => {
    const src = (node.attrs.src ?? '').trim();
    if (src === '') return null;
    const width = /(?:^|[;\s])width\s*:\s*(\d+(?:\.\d+)?)px/i.exec(node.attrs.style ?? '');
    const sized = width === null ? Number(node.attrs.width) : Number(width[1]);
    return { type: IMAGE_MARK, src, alt: node.attrs.alt ?? '', ...(Number.isFinite(sized) && sized > 0 ? { width: sized } : {}) };
  };
  const withLook = (run, look) => (look.color === undefined && look.background === undefined ? run : { ...run, ...(look.color === undefined ? {} : { color: look.color }), ...(look.background === undefined ? {} : { background: look.background }) });
  const plain = (nodes) => nodes.map((node) => node.text ?? plain(node.children)).join('');
  /** Inline content as runs; a picture is one pseudo-run, which a block settles. */
  const inline = (nodes, marks = [], look = {}) => nodes.flatMap((node) => {
    if (node.text !== undefined) return node.text === '' ? [] : [withLook({ type: 'text', text: node.text, ...(marks.length === 0 ? {} : { marks }) }, look)];
    if (node.tag === 'br') return [{ type: 'break' }];
    if (node.tag === 'img') { const image = imageOf(node); return image === null ? [] : [image]; }
    if (node.tag === 'input' || node.tag === 'wbr') return [];
    if (node.tag === 'code' && hasClass(node, 'math')) return [{ type: 'math', source: plain(node.children) }];
    const mark = Object.prototype.hasOwnProperty.call(MARK, node.tag) ? MARK[node.tag] : undefined;
    const own = node.tag === 'mark' ? { ...look, background: tintsOf(node).background ?? 2 } : { ...look, ...tintsOf(node) };
    const runs = inline(node.children, mark === undefined ? marks : [...new Set([...marks, mark])], own);
    if (node.tag !== 'a') return runs;
    const href = (node.attrs.href ?? '').trim();
    if (href === '') return runs;
    if (!keptLink(href)) { losses.add('Unsafe links were left as text.'); return runs; }
    return runs.map((run) => run.type === 'text' ? { ...run, href } : run);
  });
  /** Runs without pictures, for where a block holds text alone: a picture there is the words that describe it. */
  const textOf = (runs) => runs.flatMap((run) => {
    if (run.type !== IMAGE_MARK) return [run];
    losses.add('A picture inside a line of text was kept as its description.');
    return run.alt === '' ? [] : [{ type: 'text', text: run.alt }];
  });
  /** A line of words without the space that stood around it in the markup. */
  const trimmed = (runs) => {
    const out = runs.map((run) => ({ ...run }));
    while (out.length > 0 && out[0].type === 'text') { out[0].text = out[0].text.replace(/^\s+/, ''); if (out[0].text === '') out.shift(); else break; }
    while (out.length > 0 && out[out.length - 1].type === 'text') { const last = out[out.length - 1]; last.text = last.text.replace(/\s+$/, ''); if (last.text === '') out.pop(); else break; }
    return out;
  };
  const inlineText = (nodes) => trimmed(textOf(inline(nodes)));
  /** A table cell: paragraphs in it are lines of it. */
  const cellRuns = (nodes) => {
    const out = [];
    const add = (runs) => {
      if (runs.length === 0) return;
      if (out.length > 0 && out[out.length - 1].type !== 'break') out.push({ type: 'break' });
      out.push(...runs);
    };
    let loose = [];
    const flush = () => { add(inline(loose)); loose = []; };
    for (const node of nodes) {
      if (node.tag === 'p' || node.tag === 'div') { flush(); add(inline(node.children)); } else loose.push(node);
    }
    flush();
    return textOf(out);
  };

  // ---------------------------------------------------------------- blocks
  const piece = (type, content = [], props = {}, children = []) => ({ type, props, content, children });
  const imagePiece = (image, caption = [], look = {}, fromAlt = true) => {
    const id = `00000000-0000-4000-8000-${String(images.length + 1).padStart(12, '0')}`;
    images.push({ id, src: image.src, alt: image.alt });
    const props = { file_id: id, ...(image.width === undefined ? {} : { width: image.width }), ...(look.align === 'center' || look.align === 'right' ? { align: look.align } : {}) };
    return piece('image', fromAlt && caption.length === 0 && image.alt !== '' ? [{ type: 'text', text: image.alt }] : caption, props);
  };
  /** The block a run of inline content becomes, and the pictures in it as blocks of their own around it. */
  const paragraphs = (runs, props = {}) => {
    const out = [];
    let line = [];
    const flush = () => { if (line.length > 0) out.push(piece('paragraph', line, props)); line = []; };
    for (const run of runs) {
      if (run.type === IMAGE_MARK) { flush(); out.push(imagePiece(run)); } else line.push(run);
    }
    flush();
    return out;
  };
  const tableRows = (node) => node.children.flatMap((child) => child.tag === 'table' ? [] : child.tag === 'tr' ? [child] : child.children === undefined ? [] : tableRows(child));
  const span = (attrs, name) => {
    const number = Number(attrs[name]);
    if (attrs[name] !== undefined && (!Number.isSafeInteger(number) || number < 1)) { losses.add('An invalid table span was removed.'); return {}; }
    return number > 1 ? { [name]: number } : {};
  };
  const widthsOf = (table) => {
    const group = table.children.find((child) => child.tag === 'colgroup');
    if (group === undefined) return undefined;
    const widths = group.children.filter((child) => child.tag === 'col').map((col) => {
      const match = /width\s*:\s*(\d+(?:\.\d+)?)%/i.exec(col.attrs.style ?? '');
      return match === null ? Number.NaN : Number(match[1]);
    });
    return widths.length > 0 && widths.every((width) => Number.isFinite(width) && width > 0) ? widths : undefined;
  };
  const isBlock = (node) => node.tag !== undefined && (BLOCK.has(node.tag) || /^h[1-6]$/.test(node.tag));
  /** A checkbox that stands first in an item makes it a thing to do. */
  const boxOf = (li) => li.children.find((child) => child.tag === 'input' && (child.attrs.type ?? '').toLowerCase() === 'checkbox');
  const itemPieces = (li, type, extra) => {
    const first = li.children.findIndex((child) => isBlock(child));
    let direct = first < 0 ? li.children : li.children.slice(0, first);
    let rest = first < 0 ? [] : li.children.slice(first);
    // A loose item holds its words in a paragraph of its own.
    if (direct.every((node) => node.text !== undefined && node.text.trim() === '') && rest.length > 0 && rest[0].tag === 'p') {
      direct = rest[0].children; rest = rest.slice(1);
    }
    const box = boxOf(li);
    const todo = box !== undefined;
    const props = { ...(todo ? { checked: box.attrs.checked !== undefined } : extra), ...lookOf(li) };
    return piece(todo ? 'todo' : type, inlineText(direct), props, convert(rest));
  };
  let inColumn = 0;
  const columnsOf = (node) => {
    const columns = node.children.filter((child) => child.tag === 'div' && hasClass(child, 'column'));
    if (inColumn > 0 || columns.length < COLUMNS_MIN || columns.length > COLUMNS_MAX) return null;
    inColumn += 1;
    const made = piece('columns', [], {}, columns.map((column) => {
      const grow = /flex-grow\s*:\s*(\d+(?:\.\d+)?)/i.exec(column.attrs.style ?? '');
      return piece('column', [], grow === null || !(Number(grow[1]) > 0) ? {} : { share: Number(grow[1]) }, convert(column.children));
    }));
    inColumn -= 1;
    return made;
  };
  const convert = (nodes) => {
    const out = []; let run = [];
    const flush = () => {
      const content = inline(run);
      if (content.some((one) => one.type !== 'text' || one.text.trim() !== '')) out.push(...paragraphs(content));
      run = [];
    };
    for (const node of nodes) {
      if (node.tag === 'html' || node.tag === 'body') { flush(); out.push(...convert(node.children)); continue; }
      if (node.tag === 'head') continue;
      if (node.tag === 'img') { flush(); const image = imageOf(node); if (image !== null) out.push(imagePiece(image)); continue; }
      if (node.tag === 'a' && node.children.some((child) => child.tag === 'img')) {
        // A picture inside a link is the picture; the link is not kept.
        const found = node.children.filter((child) => child.tag === 'img' || (child.text !== undefined && child.text.trim() === ''));
        if (found.length === node.children.length) { flush(); for (const child of found) { if (child.tag === 'img') { const image = imageOf(child); if (image !== null) out.push(imagePiece(image)); } } continue; }
      }
      if (node.tag === undefined || !isBlock(node)) { run.push(node); continue; }
      flush();
      const look = lookOf(node);
      if (/^h[1-6]$/.test(node.tag)) out.push(piece('heading', inlineText(node.children), { level: Number(node.tag.slice(1)), ...look }));
      else if (node.tag === 'ul' || node.tag === 'ol') {
        let number = Number(node.attrs.start ?? 1); if (!Number.isSafeInteger(number) || number < 1) number = 1;
        for (const li of node.children.filter((child) => child.tag === 'li')) {
          out.push(itemPieces(li, node.tag === 'ul' ? 'bullet' : 'numbered', node.tag === 'ol' ? { start: number++ } : {}));
        }
      } else if (node.tag === 'table') {
        const sourceRows = tableRows(node);
        const sourceCells = sourceRows.map((row) => row.children.filter((cell) => cell.tag === 'td' || cell.tag === 'th'));
        const rows = sourceCells.map((cells) => ({ cells: cells.map((cell) => ({ content: cellRuns(cell.children), ...span(cell.attrs, 'colspan'), ...span(cell.attrs, 'rowspan'), ...tintsOf(cell) })) }));
        const widths = widthsOf(node);
        if (rows.length > 0) out.push(piece('table', rows, { header_row: sourceCells[0][0]?.tag === 'th', header_column: sourceCells.every((cells) => cells[0]?.tag === 'th'), ...(widths === undefined || widths.length !== Math.max(...rows.map((row) => row.cells.length)) ? {} : { widths }) }));
      } else if (node.tag === 'blockquote') {
        const children = convert(node.children); const first = children.shift(); out.push(piece('quote', first?.type === 'paragraph' ? first.content : [], look, first !== undefined && first.type !== 'paragraph' ? [first, ...children] : children));
      } else if (node.tag === 'aside' && hasClass(node, 'callout')) {
        const tone = ['info', 'success', 'warning', 'danger'].find((one) => hasClass(node, `tone-${one}`)) ?? 'info';
        const children = convert(node.children); const first = children.shift();
        out.push(piece('callout', first?.type === 'paragraph' ? first.content : [], { tone, ...look }, first !== undefined && first.type !== 'paragraph' ? [first, ...children] : children));
      } else if (node.tag === 'pre') {
        const code = node.children.find((child) => child.tag === 'code');
        const language = languageOf(code, node);
        const body = plain(node.children);
        if (language.toLowerCase() === 'mermaid') out.push(piece('diagram', [], { source: body }));
        else out.push(piece('code', body === '' ? [] : [{ type: 'text', text: body }], { language, ...(hasClass(node, 'wrap') ? { wrap: true } : {}) }));
      } else if (node.tag === 'hr') out.push(piece(hasClass(node, 'page-break') ? 'page_break' : 'divider'));
      else if (node.tag === 'p') {
        if (hasClass(node, 'empty') && plain(node.children).trim() === '') out.push(piece('paragraph', [], look));
        else out.push(...paragraphs(inline(node.children), look));
      } else if (node.tag === 'div' && hasClass(node, 'page-break')) out.push(piece('page_break'));
      else if (node.tag === 'div' && hasClass(node, 'math')) out.push(piece('math', [], { source: plain(node.children).trim() }));
      else if (node.tag === 'div' && hasClass(node, 'columns')) {
        const columns = columnsOf(node);
        if (columns === null) { losses.add('Columns of fewer than two or more than six were written one after another.'); out.push(...convert(node.children.flatMap((child) => (hasClass(child, 'column') ? child.children : [child])))); } else out.push(columns);
      } else if (node.tag === 'figure') {
        const image = (function find(list) { for (const child of list) { if (child.tag === 'img') return child; if (child.children !== undefined) { const deeper = find(child.children); if (deeper !== undefined) return deeper; } } return undefined; })(node.children);
        const code = (function find(list) { for (const child of list) { if (child.tag === 'code' && languageOf(child).toLowerCase() === 'mermaid') return child; if (child.children !== undefined) { const deeper = find(child.children); if (deeper !== undefined) return deeper; } } return undefined; })(node.children);
        const caption = node.children.find((child) => child.tag === 'figcaption');
        if (code !== undefined) out.push(piece('diagram', [], { source: plain(code.children) }));
        else if (image !== undefined && imageOf(image) !== null) out.push(imagePiece(imageOf(image), caption === undefined ? [] : inlineText(caption.children), look, false));
        else out.push(...convert(node.children.filter((child) => child.tag !== 'figcaption')));
      } else if (node.tag === 'details') {
        const summary = node.children.find((child) => child.tag === 'summary');
        const rest = node.children.filter((child) => child !== summary);
        const heading = summary === undefined ? undefined : summary.children.find((child) => /^h[1-6]$/.test(child.tag ?? ''));
        if (heading !== undefined) out.push(piece('heading', inlineText(heading.children), { level: Number(heading.tag.slice(1)), toggle: true, ...tintsOf(heading) }, convert(rest)));
        else {
          const title = summary === undefined ? [] : inlineText(summary.children);
          const children = convert(rest);
          if (summary === undefined && children.length > 0 && children[0].type === 'paragraph') { const first = children.shift(); out.push(piece('toggle', first.content, look, children)); } else out.push(piece('toggle', title, look, children));
        }
      } else if (node.tag === 'video' || node.tag === 'audio') {
        const src = (node.attrs.src ?? (node.children.find((child) => child.tag === 'source') ?? { attrs: {} }).attrs.src ?? '').trim();
        const caption = inlineText(node.children.filter((child) => child.tag !== 'source' && child.tag !== 'track'));
        if (/^https:\/\//i.test(src)) out.push(piece(node.tag, caption, { url: src }));
        else { losses.add('A film or a sound that is not an address on the web was left as words.'); out.push(...paragraphs(caption.length === 0 ? [{ type: 'text', text: src }] : caption)); }
      } else out.push(...convert(node.children));
    }
    flush(); return out;
  };
  return { pieces: convert(root.children), warnings: [...losses], images };
}

export { readHtml };
