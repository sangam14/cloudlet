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
| POST | `/api/v1/vmm/shutdown` | Request cancellation of the active execution |

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
directory. The legacy VMM endpoint/token does not select the execution backend.
Guest networking and the old llmman bridge are disabled in the current templates.

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
