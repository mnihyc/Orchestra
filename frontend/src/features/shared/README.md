# Shared with me

`Shared.tsx` lists the places and threads the server admits through `/shared`: things shared by name whose
parent the reader cannot browse. Readable names open their actual screens. Redacted ancestors stay unnamed;
search uses visible text only. The list supports kind and name filters, a fresh read and loading, empty and
recoverable error states. The rail and the phone's org menu use the same route availability entry.

- `Shared.tsx`: Places and threads shared from outside the trees a person can browse.

Browser cases in `tests/e2e/shared.spec.ts` cover navigation, readable and hidden paths, kind filters,
an ended share, empty results and retry after a refused read.
