# Aruna Desktop

A Tauri v2 desktop shell that runs an Aruna **user node** on the owner's own
machine and hosts the Aruna portal around it.

The app has **one window**, and that window shows the portal: the same build an
Aruna node serves at its own root, with no fork and no desktop variant. What
makes it a desktop app is what the shell injects before the portal's first
script runs, and the commands it answers afterwards.

```
window  ->  the portal artifact in ui/        (one build, web and desktop)
shell   ->  window.__ARUNA_DESKTOP__          (api base, features, bridge)
bridge  ->  node supervisor, settings, enrollment, folder picker, wipe
node    ->  a supervised aruna child process on loopback
```

## The shell context

The shell defines `window.__ARUNA_DESKTOP__` in a Tauri initialization script,
which the webview runs at document start, so the portal reads it the moment it
boots. On the web the object is absent and the same bundle behaves like any
hosted portal.

```jsonc
{
  "apiBaseUrl": "http://127.0.0.1:34116/api/v1",
  "authCallbackOrigin": "http://127.0.0.1:34115", // only with a local portal
  "features": { "desktop": true, "systemBrowserAuth": true },
  "bridge": { "invoke": "…", "version": 1 }
}
```

`version` is the command set below. A shell that answers fewer commands must not
claim version 1.

### Which API the portal talks to

| State | `apiBaseUrl` |
| --- | --- |
| enrolled **and** the node is running | `http://127.0.0.1:34116/api/v1` |
| otherwise, with a realm connected | `{realm origin}/api/v1` |
| neither | `http://127.0.0.1:34116/api/v1`, the only API this app knows |

An initialization script cannot be changed after its webview exists, and a
reload would only run the old one again. So when the choice above moves — a
realm is connected, an enrollment finishes, a device is wiped, the node starts
or stops — the shell **replaces the app window** with one carrying the new
script, on the page the old window was showing. The new window is opened before
the old one is destroyed, and it takes the next free label (`main`, `main-1`,
`main-2`, …) so it never races the teardown of the one it replaces. The check
runs after every command that can move the choice, one turn later, so the
command still answers first.

`authCallbackOrigin` is set only while the node serves its own portal listener,
because that loopback origin is the only `http` redirect URI this device can
register with a provider. See [Deep links](#deep-links) for what the app itself
accepts.

## Commands

Contract version 1. Arguments are a single object; answers are JSON. A name the
shell does not implement is rejected with `unknown command: …`, which the portal
reports as a missing bridge half rather than a failed call.

| Command | Arguments | Answers |
| --- | --- | --- |
| `node_status` | – | `{state, nodeId, realm, enrolled, apiBaseUrl, version, uptimeSeconds, message}` |
| `node_logs_tail` | `{lines}` | the last lines of the node's output, oldest first |
| `node_settings_get` | – | `{storagePath, offeredDirectories, paused, autoStart}` |
| `node_settings_set` | `{settings}` (any subset) | the stored settings |
| `enroll_apply` | `{secret, seedUrl?, realm?, label?}` | `{nodeId, realm}` |
| `wipe_device` | `{confirm}` | – |
| `pick_directory` | `{title?, startPath?}` | the chosen path, or `null` |
| `open_external` | `{url}` | – |
| `validate_realm` | `{input}` | `{origin, realm, apiVersion, portal}` |

Notes on the ones that do more than they say:

* `node_status` is also where a running enrollment is observed: it probes the
  node, advances the join, and spends the one-time secret once the node serves
  the realm. `nodeId` stays `null` — a node reports its own id only to a caller
  holding a realm token, which the shell never has, so the portal reads it with
  the owner's own.
* `node_settings_set` stops the node when `paused` turns on and starts it when
  it turns off. Every other field reaches the node on its next start. Offered
  directories are stored for the portal; realm nodes pull them from the node.
* `enroll_apply` needs only the secret. The seed URL and the realm travel inside
  it; the shell keeps its own copy of both only to watch the join. It clears a
  pause, because enrolling is an explicit "run this node".
* `wipe_device` re-checks `confirm` against `wipe`, stops the node, deletes the
  data directory the shell provisioned (and a storage directory outside it),
  clears the enrollment, the offered directories and the storage override, and
  keeps the machine configuration. A path that is not deep enough to be one of
  ours is refused rather than deleted. The node's own REST wipe is a separate
  surface.
* `validate_realm` checks the address, **remembers it** as this device's realm
  and answers with the origin. Remembering is the point: it is what the portal
  talks to until a node of this device's own is running.
* `open_external` opens `http` and `https` in the system browser and refuses
  every other scheme.

## The portal artifact

`ui/` holds the portal build the window serves. It is gitignored, and it is the
same artifact a node serves, installed from the website repository:

```bash
scripts/sync-frontend.sh              # ../website, or $ARUNA_WEBSITE_DIR
```

The directory is called `ui` rather than `dist` because `dist` would read as
this repository's own build output, which it is not: it is someone else's build,
vendored in. Packaged builds bundle exactly what that script installs.

Without it, the build writes a placeholder page naming the command
(`src-tauri/src/fallback.html`, installed by `build.rs`), so a checkout that has
never synced a portal still builds and still explains itself.

### Development

Linux needs the Tauri v2 prerequisites: `webkit2gtk-4.1`, `gtk3`,
`libayatana-appindicator`, `librsvg`, and `patchelf`. This repository has no npm
tooling of its own.

Two ways to run it:

```bash
# 1. against the website's dev server, with hot reload
(cd ../website && npx vite --port 1420 --strictPort)
ARUNA_DESKTOP_NODE_BIN=/path/to/aruna cargo run --manifest-path src-tauri/Cargo.toml

# 2. against the installed artifact, as a packaged build serves it
scripts/sync-frontend.sh
ARUNA_DESKTOP_NODE_BIN=/path/to/aruna \
  cargo run --manifest-path src-tauri/Cargo.toml --release --features custom-protocol
```

The `custom-protocol` feature is what embeds `ui/`; without it Tauri loads the
dev URL, whatever the profile says. The Tauri CLI turns it on for `tauri build`
by itself.

On Wayland the app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` before the webview
starts, unless that variable or `ARUNA_DESKTOP_NO_WAYLAND_WORKAROUND` is already
set.

Checks:

```bash
cargo check --all-targets --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo +nightly fmt --all --manifest-path src-tauri/Cargo.toml -- --check
```

## Sidecar model

The node is a **child process**, not a library. `src-tauri/src/node/` resolves a
node binary, writes its configuration, starts it, tails its output, restarts it
after an unexpected exit, and stops it with `SIGTERM` followed by a kill once
the node's own shutdown grace has passed.

Binary resolution, most specific first:

1. `ARUNA_DESKTOP_NODE_BIN` (development override),
2. the path stored in the settings file,
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
| `CORS_ALLOWED_ORIGINS` | every origin the app window can carry, and the node's own portal |
| `ARUNA_SHUTDOWN_GRACE_SECS` | `20`; the node rejects anything below 16 |
| `PORTAL_MODE` | `artifact` with a portal directory set, otherwise `disabled` |
| `PORTAL_DIR` | the portal artifact directory (portal mode only) |
| `PORTAL_SOCKET_ADDRESS` | `127.0.0.1:34115` (portal mode only) |
| `S3_ADDRESS`, `S3_HOST` | the loopback endpoint while the local S3 listener is on, **empty** while it is off |
| `S3_PUBLIC_URL` | only while the local S3 endpoint is enabled |
| `ONBOARDING_SECRET` | the one-time enrollment secret, consumed on the next start |

The ports are fixed on loopback so the portal origin and the OIDC redirect URI
can be registered up front.

`CORS_ALLOWED_ORIGINS` carries four origins, because the portal calls the node's
API from whichever one the window happens to have:

| Origin | When |
| --- | --- |
| `tauri://localhost` | packaged builds, everywhere but Windows |
| `http://tauri.localhost` | packaged builds on Windows |
| `http://localhost:1420` | a dev run against the website's vite server |
| `http://127.0.0.1:34115` | the node's own portal listener |

Two rows of the env table are the device profile itself:

* an **empty** `S3_HOST` and `S3_ADDRESS` is how the node is told to serve no S3
  listener. They are written empty rather than omitted, because an omitted key
  leaves an inherited or `.env`-provided one in place. Setting only one of the
  pair aborts the node, so they always move together.
* `PORTAL_MODE` is written as `disabled` when no portal directory is set. It is
  never written empty: aruna accepts only `artifact` and `disabled`.

## Deep links

The `aruna` scheme is registered by the app (`tauri-plugin-deep-link`), and
`tauri-plugin-single-instance` forwards a second launch to the running window.
Both accept the authority-less form (`aruna:enroll?…`). Unknown routes are
dropped with a warning that names the route only, never the query.

| Link | Behaviour |
| --- | --- |
| `aruna://enroll?secret=…&seed=…&realm=…` | the enrollment is armed, the window opens `/app/device`, and `enroll-invite` is emitted |
| `aruna://auth/callback?…` | the window opens `/auth/callback` with the query **and** the fragment the provider returned |

`secret` and `seed` are required in a link, and `seed` must be an `http`/`https`
origin; a link missing either is dropped. The owner already accepted the
invitation by following it out of the portal, so the shell arms it rather than
stashing it for a second confirmation.

### `enroll-invite`

```jsonc
{ "seed": "https://seed.example", "realm": "r-1", "applied": true, "error": null }
```

The secret is never part of it. The event is a **hint**, not the source of
truth: a link followed into a cold start can deliver it before the portal is
listening, which is why the window also navigates to `/app/device`, where
`node_status` reports the same enrollment either way.

## Enrollment

The seed mints the secret and the deep link; `aruna/api/src/routes/onboarding.rs`
builds exactly the three query keys above.

The node needs **only `ONBOARDING_SECRET`**. The seed URL, the realm id, the
enrollment id and the onboarding mode all travel inside the secret itself
(`core/src/onboarding.rs`, postcard plus URL-safe base64), which the joiner
decodes to reach `{seed}/api/v1/onboarding/bootstrap`.

The secret is stored once, is never sent to the webview, never logged, and is
cleared the moment the node serves the realm. It must not be cleared earlier: a
node whose join is half done needs it again to resume.

### The two signals

The device cannot poll `GET {seed}/api/v1/onboarding/secrets/{id}/status`: the
deep link carries no enrollment id, and the route needs a realm bearer token.
Enrollment progress is therefore observed, never queried:

1. `GET http://127.0.0.1:34117/readyz` turning `200`. The node binds its
   listeners only after the join has succeeded, and nothing is logged when it
   does, so readiness is the authoritative "the seed accepted this device".
2. `GET http://127.0.0.1:34116/api/v1/info` reporting the `realm_id` from the
   invitation. The seed is asked for it when a pasted secret named no realm.

The join walks `received -> starting -> waiting -> visible -> done`, and only
ever moves forward, so a blipping probe cannot rewind it.

### Failures

The joiner logs nothing about the join through `tracing`. On a refused secret it
prints one line to stderr and exits with code 1, so the supervisor's stderr tail
is scanned for it and turned into the `message` of `node_status`:

| Node output | Reported as |
| --- | --- |
| `onboarding bootstrap failed: bootstrap endpoint returned 401 Unauthorized` | the secret is expired, already used, or invalid |
| `onboarding bootstrap failed: bootstrap endpoint returned 409 Conflict` | the realm will take no further device for this owner |
| `onboarding mode mismatch between secret and bootstrap response` | the secret was minted for another kind of node |
| `… requires ONBOARDING_SECRET to refresh bootstrap material` | an interrupted join; a fresh secret is needed |
| `… waiting for onboarding placement for node …` | accepted but never placed |

A node that cannot stay up fails the enrollment once the supervisor gives up.

## Sign-in

Sign-in leaves through the system browser (`systemBrowserAuth`, RFC 8252): the
portal hands the authorization URL to `open_external`, and the app is brought
back by `aruna://auth/callback`, which lands on the portal's own callback route
inside the app window.

Which redirect URI a realm registers is still open, and it is a portal and
provider decision rather than a shell one. The shell accepts the deep link
whatever else is configured, and only claims the loopback portal origin through
`authCallbackOrigin` while the node actually serves that portal.

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
