# Aruna Desktop

A Tauri v2 desktop shell that runs an Aruna **user node** on the owner's own
machine and gives them a native control surface for it.

The app is not a second portal. It owns two windows:

* the **control window** (the Vue frontend in `src/`) for node status, device
  enrollment, and settings, and
* the **portal window**, which navigates to the node's own portal listener at
  `http://127.0.0.1:34115` as a top-level window. The node sends
  `frame-ancestors 'none'`, so the portal is never embedded in an iframe.

## Sidecar model

The node is a **child process**, not a library. `src-tauri/src/node/` resolves a
node binary, writes its configuration, starts it, tails its output, restarts it
after an unexpected exit, and stops it with `SIGTERM` followed by a kill once
the node's own shutdown grace has passed.

Binary resolution, most specific first:

1. `ARUNA_DESKTOP_NODE_BIN` (development override),
2. the path stored in the settings view,
3. `aruna` next to the desktop executable (the bundled sidecar),
4. `<resources>/binaries/aruna`.

Nothing is bundled yet, so a fresh install reports `No node binary` with the
list of paths it searched, and the rest of the app keeps working.

### Configuration contract

Settings are typed (`NodeSettings`) and rendered into a `.env` file inside the
node's working directory (`<app data>/node/.env`), which is also passed to the
child as environment variables: the node reads its configuration through
`dotenvy`, which never overrides an inherited value, so the desktop sets both
and a stale shell environment cannot leak in.

| Key | Value written by the app |
| --- | --- |
| `STORAGE_PATH` | `<app data>/node/storage`, or the configured path |
| `SOCKET_ADDRESS` | `127.0.0.1:34116` (REST API) |
| `OPS_SOCKET_ADDRESS` | `127.0.0.1:34117` (health, readiness, metrics) |
| `P2P_SOCKET_ADDRESS` | `0.0.0.0:34119`, deliberately not loopback so iroh stays reachable |
| `API_PUBLIC_URL` | `http://127.0.0.1:34116` |
| `CORS_ALLOWED_ORIGINS` | `http://127.0.0.1:34115`, the portal origin exactly |
| `ARUNA_SHUTDOWN_GRACE_SECS` | `20`; the node rejects anything below 16 |
| `PORTAL_MODE` | `artifact` with a portal directory set, otherwise `disabled` |
| `PORTAL_DIR` | the portal artifact directory (portal mode only) |
| `PORTAL_SOCKET_ADDRESS` | `127.0.0.1:34115` (portal mode only) |
| `S3_ADDRESS`, `S3_HOST`, `S3_PUBLIC_URL` | only while the local S3 endpoint is enabled |
| `ONBOARDING_SECRET` | the one-time enrollment secret, consumed on the next start |

The ports are fixed on loopback so the portal origin and the OIDC redirect URI
can be registered up front. They are editable in the settings view for the rare
clash.

### Waiting on aruna

Stock aruna still expects a full server profile, so two things are known gaps
rather than bugs here:

* `S3_HOST` and `S3_ADDRESS` are mandatory in aruna's configuration, so a device
  profile with the S3 listener switched off cannot start until the optional
  listener work lands. Enable the local S3 endpoint in the settings meanwhile.
* the node aborts when no `.env` file exists, which is why the file is always
  written even though the environment already carries every value.

Device enrollment status, the device list, and the portal's OIDC callback path
are marked `TODO(aruna)` in the sources; they arrive with the user-node routes.

## Deep links

The `aruna` scheme is registered by the app (`tauri-plugin-deep-link`), and
`tauri-plugin-single-instance` forwards a second launch to the running window.

| Link | Behaviour |
| --- | --- |
| `aruna://enroll?secret=…&realm=…&name=…&endpoint=…` | the payload is stashed, the enroll view opens with the secret prefilled |
| `aruna://auth/callback?…` | the query is forwarded to the portal window, which owns the OIDC exchange |

Both accept the authority-less form (`aruna:enroll?…`) as well. Unknown routes
are dropped with a warning that names the route only, never the query.

## Development

Linux needs the Tauri v2 prerequisites: `webkit2gtk-4.1`, `gtk3`,
`libayatana-appindicator`, `librsvg`, and `patchelf`.

```bash
npm install
ARUNA_DESKTOP_NODE_BIN=/path/to/aruna npm run tauri dev
```

Checks:

```bash
npx vue-tsc -b --pretty false
npx vite build
cargo check --all-targets --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

### Depending on aruna crates

The supervisor shells out, so this repository has no aruna dependency today.
When one becomes necessary, it is pinned as a git dependency on a named branch
and rev, following the paired-branch convention already used between irokle and
craqle: a desktop branch names the aruna branch it was built against, and both
move together. Path dependencies must not be used, because they break the
container and release builds.

## License

Dual licensed under [Apache 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), matching
the aruna repository.
