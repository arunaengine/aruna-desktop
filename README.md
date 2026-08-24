# Aruna Desktop

A Tauri v2 desktop shell that runs an Aruna **user node** on the owner's own
machine and gives them a native control surface for it.

The app is not a second portal. It owns two windows:

* the **control window** (the Vue frontend in `src/`) for the realm connection,
  node status, device enrollment, and settings, and
* the **portal window**, which navigates to a portal as a top-level window.
  Portals send `frame-ancestors 'none'`, so one is never embedded in an iframe.

## Portal window

The portal window has two possible targets:

| Target | Address | Chosen when |
| --- | --- | --- |
| the realm | the remembered realm origin | by default |
| this node | `http://127.0.0.1:34115` | the owner ticks the offline portal in the settings, or picks it after the realm turned out to be unreachable |

The local portal counts as usable only while the node is enrolled, ready, and
configured with a portal directory. The app never switches targets on its own:
an unreachable realm **offers** the local portal, it does not fall back to it.

Every open probes its target first (`src-tauri/src/realm.rs`), because
WebKitGTK, and therefore Tauri, reports no page-load failure the app could hook.
A refused connection, a rejected certificate, an unresolvable host or an origin
that serves no Aruna node becomes an in-app error page with the reason and a
retry, never the webview's own failure page.

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
| `S3_ADDRESS`, `S3_HOST` | the loopback endpoint while the local S3 listener is on, **empty** while it is off |
| `S3_PUBLIC_URL` | only while the local S3 endpoint is enabled |
| `ONBOARDING_SECRET` | the one-time enrollment secret, consumed on the next start |

The ports are fixed on loopback so the portal origin and the OIDC redirect URI
can be registered up front. They are editable in the settings view for the rare
clash.

Two of those rows are the device profile itself:

* an **empty** `S3_HOST` and `S3_ADDRESS` is how the node is told to serve no S3
  listener. They are written empty rather than omitted, because an omitted key
  leaves an inherited or `.env`-provided one in place. Setting only one of the
  pair aborts the node, so they always move together.
* `PORTAL_MODE` is written as `disabled` when no portal directory is set. It is
  never written empty: aruna accepts only `artifact` and `disabled`, and an
  empty value is a configuration error rather than a default.

The `.env` file is no longer mandatory for the node, which starts fine on the
inherited environment alone. It is still written, because it is what an owner
inspects when a start goes wrong, and `dotenvy` never overrides a value the
desktop already set.

## Deep links

The `aruna` scheme is registered by the app (`tauri-plugin-deep-link`), and
`tauri-plugin-single-instance` forwards a second launch to the running window.

| Link | Behaviour |
| --- | --- |
| `aruna://enroll?secret=…&seed=…&realm=…` | the invitation is stashed in the backend and the enroll view opens |
| `aruna://auth/callback?…` | the query is forwarded to the portal window, which owns the OIDC exchange |

The callback is sent to the origin the portal window is currently showing, so a
login redirect follows the portal it was started from. It is dropped when no
portal window is open.

Both accept the authority-less form (`aruna:enroll?…`) as well. Unknown routes
are dropped with a warning that names the route only, never the query.

`secret` and `seed` are required and `seed` must be an `http`/`https` origin;
a link missing either is dropped. `realm` is a label, and the seed can be asked
for it when a paste carries none.

The portal's OIDC callback path is still marked `TODO(aruna)`: it mirrors the
loopback redirect URI until the website auth seam names it.

## Enrollment

The seed mints the secret and the deep link; `aruna/api/src/routes/onboarding.rs`
builds exactly the three query keys above.

The node needs **only `ONBOARDING_SECRET`**. The seed URL, the realm id, the
enrollment id and the onboarding mode all travel inside the secret itself
(`core/src/onboarding.rs`, postcard plus URL-safe base64), which the joiner
decodes to reach `{seed}/api/v1/onboarding/bootstrap`. There is no seed, realm
or node-kind environment key to set, and none exists.

The desktop keeps the seed URL and the realm id anyway, because it polls them.
The secret is stored once, is never sent to the webview, never logged, and is
cleared from the settings the moment the node serves the realm. It must not be
cleared earlier: a node whose join is half finished needs it again to resume.

### The two signals

The device cannot poll `GET {seed}/api/v1/onboarding/secrets/{id}/status`: the
deep link carries no enrollment id, and the route needs a realm bearer token.
Enrollment progress is therefore observed, never queried:

1. `GET http://127.0.0.1:34117/readyz` turning `200`. The node binds its
   listeners only after the join has succeeded, and nothing is logged when it
   does, so readiness is the authoritative "the seed accepted this device".
2. `GET http://127.0.0.1:34116/api/v1/info/realm` reporting the `realm_id` from
   the invitation. `GET {seed}/api/v1/info/realm` answers anonymously too, but
   it only fills its `nodes` list for a caller holding a realm token, so the
   realm id is the field a token-less desktop can use. The seed is asked for it
   when a pasted secret named no realm.

The view walks `received -> starting -> waiting -> visible -> done`, and the
progress only ever moves forward, so a blipping probe cannot rewind it.

### Failures

The joiner logs nothing about the join through `tracing`. On a refused secret it
prints one line to stderr and exits with code 1, so the supervisor's stderr tail
is scanned for it and turned into a targeted message:

| Node output | Shown as |
| --- | --- |
| `onboarding bootstrap failed: bootstrap endpoint returned 401 Unauthorized` | the secret is expired, already used, or invalid |
| `onboarding bootstrap failed: bootstrap endpoint returned 409 Conflict` | the realm will take no further device for this owner |
| `onboarding mode mismatch between secret and bootstrap response` | the secret was minted for another kind of node |
| `… requires ONBOARDING_SECRET to refresh bootstrap material` | an interrupted join; a fresh secret is needed |
| `… waiting for onboarding placement for node …` | accepted but never placed |

A node that cannot stay up fails the enrollment once the supervisor gives up.

## Development

Linux needs the Tauri v2 prerequisites: `webkit2gtk-4.1`, `gtk3`,
`libayatana-appindicator`, `librsvg`, and `patchelf`.

```bash
npm install
ARUNA_DESKTOP_NODE_BIN=/path/to/aruna npm run tauri dev
```

On Wayland the app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` before the webview
starts, unless that variable or `ARUNA_DESKTOP_NO_WAYLAND_WORKAROUND` is already
set.

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
