# Cloudlet + BoxLite

A local sandbox console: React, TypeScript, shadcn/ui, and a Rust API with
**BoxLite 0.10.0 as its embedded primary runtime**. The custom TAP-based VMM
is retained as legacy source, not used by the dashboard.

## Build and run

Build prerequisites: current Rust, Node.js 22.12+ with npm, C/C++ tools,
pkg-config, and protoc. Cargo downloads BoxLite's official platform runtime
bundle on the first build. Cargo's offline flag does not prevent downloads
performed by BoxLite's build script.

```sh
npm --prefix frontend ci
npm --prefix frontend run build
cargo build --release -p cloudlet
./target/release/cloudlet doctor
./target/release/cloudlet dashboard
```

Open [the console](http://127.0.0.1:3000/). No Node.js, separate broker,
frontend directory, kernel build, FUSE rootfs builder, or CAP_NET_ADMIN is
needed at runtime for these templates. One executable is distributed, not one
process: BoxLite extracts helpers/firmware and stores OCI images and guest disks.

## Linux setup

Run as your ordinary user with read/write access to /dev/kvm. BoxLite performs
a real KVM smoke test at initialization. From a normal host terminal:

```sh
ls -l /dev/kvm
id -nG
test -r /dev/kvm && test -w /dev/kvm
./target/release/cloudlet doctor
```

If missing, check firmware virtualization and the appropriate KVM kernel module.
If the device is owned by group kvm, an administrator can add your user to that
group; log out and back in. Do not make KVM world-writable or run the console
as root. Restricted containers may hide KVM even when the host supports it.
Use a host terminal or an explicitly device-enabled environment; do not bypass
no_new_privs. Restart Cloudlet after fixing access.

The UI stays available if runtime initialization fails and shows diagnostics.
Initialization readiness is not proof of OCI guest boot; the execution stream
reports when that happens.

## Runtime policy

- Fixed OCI templates: Rust 1.90, Python 3.13, Node.js 22 on Debian Bookworm.
- Guest execution as UID/GID 65534. No host mounts, credentials, or published ports.
- Guest ingress and egress disabled. Host-side OCI pulls still need Internet access.
- BoxLite default jailer/seccomp/namespaces enabled, never silently disabled.
- Two vCPUs, 1024 MiB RAM, 8 GiB requested sparse guest disk. This is not a strict
  host disk quota: the base image can require a larger disk.
- One execution across clients; 60-second command / 10-minute cold-start deadline.
- Application output cap 1 MiB and bounded SSE queue; slow consumers cancel.
  The SDK has internal buffering, so this is not a hard host-memory quota.
- Completion, cancellation, disconnect, or timeout triggers guest stop.
  Cleanup failure blocks another execution until resolved.
- Retained stopped disks and persistent metadata; at most 100 sandboxes.
  Remove deletes a stopped guest disk permanently, never force-deleting live boxes.

Image tags are fixed, not immutable digests. Host cgroup enforcement is not
independently verified. This is a local development control plane, not a
reviewed hostile multi-tenant execution service.

## Configuration

| Variable | Default / purpose |
| --- | --- |
| CLOUDLET_API_HOST | 127.0.0.1 |
| CLOUDLET_API_PORT | 3000 |
| CLOUDLET_BOXLITE_HOME | $XDG_DATA_HOME/cloudlet/boxlite or ~/.local/share/cloudlet/boxlite |
| CLOUDLET_API_AUTH_TOKEN | Optional on loopback; 16+ printable non-space characters |
| CLOUDLET_API_ALLOW_REMOTE | Explicit non-loopback opt-in; also requires a token |

One process owns each BoxLite state directory. Do not point Cloudlet at an
unrelated BoxLite store. Legacy VMM endpoint/token settings do not select the
execution backend. The browser stores API tokens in tab memory only. Remote
use needs trusted TLS and additional review. Same-origin and loopback Host
guards remain enforced. CSP permits inline styles for Radix positioning/modal
scroll locking, but not inline scripts or eval.

## Console and API

Dashboard metrics report guest allocations, not host capacity. Sandboxes shows
persistent inventory with stop/remove controls. Activity streams preparation,
stdout/stderr, exit status, and cleanup errors. Output history is tab-local;
download before reloading.

| Method | Route | Purpose |
| --- | --- | --- |
| GET | /healthz | Public API/runtime initialization status |
| GET | /api/v1/overview | Authenticated diagnostics and persistent inventory |
| POST | /api/v1/workloads | Rust/Python/Node source execution, SSE |
| POST | /api/v1/sandboxes/{id}/stop | Stop exact sandbox, including another tab's execution |
| DELETE | /api/v1/sandboxes/{id} | Remove stopped sandbox, never force |
| POST | /api/v1/vmm/shutdown | Compatibility alias for active execution cancellation |

See [API contract](src/api/README.md) for request shapes. Snapshot/clone,
arbitrary OCI image/command execution, and model management are not exposed.

**Models:** BoxLite supplies sandbox compute, not inference. The legacy llmman
bridge is not attached to these network-disabled guests. Model connectivity
requires an explicit outbound/secret policy; no models or providers are installed.

## Architecture

```text
cloudlet executable
├── Embedded React + shadcn/ui assets → browser
├── Rust HTTP API ← same-origin requests / bearer policy
│   ├── Shared control-plane contracts
│   ├── Admission / output / cancellation / cleanup policy
│   └── Embedded BoxLite SDK
│       ├── OCI cache + SQLite inventory + retained disks
│       └── Isolated shim subprocess → libkrun / KVM → OCI guest
└── Optional legacy-vmm feature (not used by dashboard)
```

Frontend primitives live in frontend/src/components/ui. Runtime policy lives
in src/api/src/runtime.rs; HTTP/auth/SSE in service.rs; shared contracts in
src/control-plane; executable dispatch in src/cloudlet. Legacy src/vmm,
src/agent, and src/fs-gen remain separate workspace crates.
[Historical setup](docs/legacy-vmm.md) is reference only.

## Development and verification

Run the API and npm --prefix frontend run dev in separate terminals. Vite
proxies API calls to port 3000. Rebuild frontend then Rust for release changes.

```sh
npm --prefix frontend test
npm --prefix frontend run build
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

KVM-independent tests do not replace running templates on a real host.
BoxLite's upstream build script can print informational cargo:warning lines
while embedding assets; these are separate from Cloudlet compiler lints.
