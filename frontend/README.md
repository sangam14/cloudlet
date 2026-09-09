# Cloudlet console

Local React 19 / TypeScript / shadcn/ui interface for the Rust/BoxLite control plane,
inspired by AgentKernel's management layout. This is not a fork of AgentKernel
and does not add its unsupported backend features.

From the repository root:

```sh
npm --prefix frontend ci
npm --prefix frontend run build
cargo build --release -p cloudlet
./target/release/cloudlet dashboard
```

Open the printed URL (default http://127.0.0.1:3000/). `src/api` embeds `dist/`
at compile time; rebuilding the frontend alone does not change an existing
Rust binary. A fresh checkout must build the frontend before building any
Rust target that depends on `api`. Node is not a runtime dependency.

For development, leave the API running and use `npm --prefix frontend run dev`.
Vite proxies `/api` and `/healthz` to the local API on port 3000. Keep this
development server loopback-only. Run transport tests with
`npm --prefix frontend test`; `npm --prefix frontend run build` typechecks.

- `src/App.tsx`: navigation, templates, dialogs, session history, execution views.
- `src/RuntimeInventory.tsx`: persistent BoxLite inventory and confirmed lifecycle actions.
- `src/Models.tsx`: llmman inventory, confirmed downloads, prompts, unload, and latest operation output.
- `src/components/ui/`: shared shadcn/Radix UI primitives.
- `src/api.ts`: authenticated same-origin fetch, request contract, bounded SSE decoder.
- `src/styles.css` and `src/tokens.css`: shared visual system and responsive layout.
- `src/api.test.ts`: streaming, state transitions, errors and request-contract tests.

The API token is held only in React state, never URLs or browser storage.
Rust, Python, and Node.js source templates execute inside BoxLite guests.
Dashboard metrics report live guest allocations, not host capacity. Sandboxes
shows persistent runtime inventory, while Activity output remains tab-local.
Closing an execution stream requests guest cleanup. Stop/remove actions require
confirmation; removing a stopped sandbox permanently deletes its guest disk.
Models use the Rust API's fixed-loopback llmman adapter, independently of KVM
readiness. Inference runs on the host; these templates still have no network or
guest model bridge. No model provider key is accepted by the UI. Downloading
weights requires confirmation; llmman may separately download its inference
engine on startup or first use. Model replies come from the API, never fixture
data. The root README contains screenshots of all six console pages.
