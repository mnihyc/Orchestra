# Shipped facts sources

This extension bundle links the shipped source modules and returns their common
registered table. Both server and connector use it; the CLI uses its registered
names when validating local plugins. A new source adds its own extension and a
bundle entry, without a vendor branch or closed source list in a core crate.
Names remain unique. Intrinsic framework roles remain reserved independently of
provider names. Each source owns its exact hosts, optional administrative fields,
reported members and documentation; no consumer guesses a balance from a vendor.
