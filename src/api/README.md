# Cloudlet HTTP control plane

Build the frontend before compiling this crate. From the repository root:

```sh
npm --prefix frontend ci
npm --prefix frontend run build
cargo run --bin api
```

The default origin is http://127.0.0.1:3000. `cloudlet dashboard` runs the same
API, embedded React/shadcn assets, and BoxLite runtime. No separate broker is used.

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/` | Embedded browser console |
| GET | `/healthz` | Public API and BoxLite initialization status |
| GET | `/api/v1/overview` | Runtime diagnostics and persistent sandbox inventory |
| POST | `/api/v1/workloads` | Submit Rust/Python/Node source; stream execution events (SSE) |
| POST | `/api/v1/sandboxes/{id}/stop` | Stop an exact sandbox, including another tab's execution |
| DELETE | `/api/v1/sandboxes/{id}` | Remove a stopped sandbox and guest disk, never force |
| POST | `/api/v1/workloads/cancel` | Request cancellation of the active BoxLite execution |
| GET | `/api/v1/models` | llmman readiness, inventory, and latest operation |
| POST | `/api/v1/models/operations` | Start a model pull, prompt, or unload operation |

`/run` and `/shutdown` remain compatibility aliases. `/configuration`,
`/logs/{id}`, and `/metrics/{id}` are not implemented.

When `CLOUDLET_API_AUTH_TOKEN` is configured, control requests require
`Authorization: Bearer <token>`. Static assets and `/healthz` remain public.
Cross-origin browser control requests are rejected; loopback binds reject
foreign Host names to limit DNS-rebinding attacks. Non-browser clients without
Origin remain supported. Remote binds require explicit opt-in and a token;
use TLS before sending credentials over an untrusted network.

BoxLite runs in-process with extracted helper subprocesses. Linux requires
read/write KVM access. `CLOUDLET_BOXLITE_HOME` selects its persistent state
directory. BoxLite is the only sandbox backend; no separate VMM broker, custom
gRPC client, alternate runtime feature, or broker credentials remain. BoxLite
still uses its own internal host/guest protocol.
Guest networking is disabled in the current templates. The model adapter is
independent host inference, not guest-to-model networking. `/healthz` reports
`runtime_endpoint: "embedded://boxlite"`; the former `vmm_endpoint` field and
`/api/v1/vmm/shutdown` route have been removed. Use `/api/v1/workloads/cancel`.

## Model operations

`CLOUDLET_LLMMAN_URL` defaults to `http://127.0.0.1:17434`. Only an HTTP loopback
IP endpoint without credentials, extra path, query, or fragment is accepted.
Start llmman separately; Cloudlet never executes a host command to manage it.
The adapter enforces the same Cloudlet auth/origin policy, disables redirects
and proxy discovery, sends the llmman node-local hop header, and never forwards
Cloudlet bearer credentials. llmman's own port remains unauthenticated.

```json
{"action":"run","model":"docker.io/ai/your-existing-model:latest","prompt":"Hello"}
```

Actions are `pull`, `run`, and `unload`. The model must appear in the local store
before `run`; `unload` also accepts loaded models no longer present on disk.
`pull` accepts a model name or OCI/Hugging Face reference, not an arbitrary URL
or file path. Pull/unload do not require a prompt. Additional fields are rejected.

Accepted operations return HTTP 202 with `{"accepted":true}`. Poll
`GET /api/v1/models`; its shape is:

```json
{
  "ready": true,
  "endpoint": "http://127.0.0.1:17434",
  "detail": "llmman connected · host inference · guests remain network-isolated",
  "models": [],
  "operation": null
}
```

Each model has `name`, `size_bytes`, `loaded`, and `stored`. An operation has
`action`, `model`, `status`, `detail`, and `output`. Status is `running`, `done`,
`failed`, or `unconfirmed`. Confirmed upstream failures permit retry. Unknown
completion blocks new operations until the operator verifies llmman is idle and
restarts Cloudlet. Reconnecting a browser does not clear this guard. Only the
latest operation is stored in API memory; there is no durable conversation log.

Requests use at most 8 KiB prompt text and 256 generated tokens. The adapter
bounds JSON replies to 1 MiB and streamed download progress to 16 KiB per frame.
One operation runs at a time across Cloudlet clients, separate from the sandbox
queue. Downloads may continue upstream after a 30-minute timeout or disconnect;
generation and unloading have ten-minute timeouts. An unload acknowledgment is
not independent verification of process exit. See the root README for setup,
privacy, upstream compatibility, and real screenshots.

## Sandbox operations

Example workload body:

```json
{
  "workload_name": "hello",
  "language": "rust",
  "code": "fn main() { println!(\"Hello\"); }",
  "log_level": "info",
  "action": "prepare-and-run",
  "server": { "address": "localhost", "port": 50051 },
  "build": { "source-code-path": "/unused-by-api", "release": true }
}
```

The `server` and `build` fields are compatibility fields; BoxLite template policy
chooses the guest configuration. Names use 1–64 lowercase letters, digits, or
hyphens. Source is capped at 240 KiB, JSON at 256 KiB by default. Stream stages
are `Pending`, `Building`, `Running`, `Done`, `Failed`, and `Debug`, with optional
`stdout`, `stderr`, and `exit_code`. With `raw_output: true`, output fields are
chunks: preserve them exactly without appending newlines. A terminal event is
emitted after guest cleanup; a cleanup failure is reported as `Failed`.

See the repository README for limits and host setup. Runtime initialization
readiness is not proof of an OCI workload boot. Output history is browser-local;
the runtime retains sandbox metadata and stopped disks, not console history.
