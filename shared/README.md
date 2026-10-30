# Shared document conversion

`document-html.js` is the inert HTML converter imported by the browser and embedded verbatim by the native document importer. It creates no browser document, evaluates no input, and has no file, network or process access. It keeps nested lists and table headers, removes active markup and returns explicit loss warnings.

`document-html.d.ts` names its ordinary block pieces for the browser. Native code validates those same wire values before the normal document producer accepts them.
