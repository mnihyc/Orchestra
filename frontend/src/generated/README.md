# Generated contracts

Files that `cargo xtask contracts` writes from the Rust contracts. Do not edit them by hand: change the Rust source and generate them again.

| File | What it holds |
|---|---|
| `api-types.ts` | The browser API's request and response types, made from the OpenAPI input of the API contracts |
| `settings-registry.ts` | The catalogue of settings: keys, kinds, defaults and scopes |
| `permissions.ts` | Empty. It is reserved for permission definitions and holds no contract |

`cargo xtask contracts --check` generates beside the committed files and fails when they differ, which is how CI keeps them current. Generating the types needs `npm ci` in `frontend/` first.
