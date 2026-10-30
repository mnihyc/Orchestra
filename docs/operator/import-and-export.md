# Import and export

A place export can write an Orch package for import. Imports are planned before a
job starts. A failed or
cancelled job retains its partial results and report. Imported objects follow the
destination's permissions; restoring package sharing additionally requires access
management and follows the destination's locks.

`import.max_bytes` and `import.max_items` are inherited settings. When unset they
follow the destination's export limits, which ship as 2 GiB and 10,000 items. Set
these values at the appropriate scope and lock them only when needed. The review
shows the resolved limits and blockers. `[limits] imports_per_user` controls how
many imports one person can run at once; its shipped value is two.

An import and its report remain available to the person who made it. Undo uses the
ordinary soft-deletion retention: its plan shows when affected work can be brought
back. Undo stays unavailable when the affected work or its access cannot be checked.
See [Import and export](../user/import-and-export.md) for the person's workflow.

Import lists retain one page at a time. Report downloads write each received chunk
to a file chosen by the reader, so the page does not keep the report's contents.
A browser without direct file saving cannot download a report from this screen;
the format choices explain this limitation. The report remains readable in pages.
