/** Starts the conversion of a Word document held in `__docx` (an ArrayBuffer) to HTML by the vendored mammoth, in the sandbox of the native importer: the answer is `__state` once `done` is true. Nothing here reads a file or opens an address. */

// How Word's own styles are read where mammoth has no name for them: a title is a first-level heading, an underline is underlined, a page break is the line that stands for one.
const STYLE_MAP = [
  "p[style-name='Title'] => h1:fresh",
  "p[style-name='Subtitle'] => p:fresh",
  "u => u",
  "strike => s",
  "br[type='page'] => hr.page-break:fresh",
];

globalThis.__state = { done: false, html: '', messages: 0, error: null };

globalThis.__start = function start() {
  const state = globalThis.__state;
  try {
    mammoth.convertToHtml({ arrayBuffer: globalThis.__docx }, { styleMap: STYLE_MAP }).then(
      (result) => { state.html = result.value; state.messages = result.messages.length; state.done = true; },
      (error) => { state.error = String((error && error.message) || error); state.done = true; },
    );
  } catch (error) {
    state.error = String((error && error.message) || error); state.done = true;
  }
};
