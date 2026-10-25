# Aruna Desktop

A Tauri v2 desktop shell that runs an Aruna **user node** on the owner's own
machine and hosts the Aruna portal around it.

The app has **one window**, opened once and kept for the life of the app, and
that window shows the portal: the same build an Aruna node serves at its own
root, with no fork and no desktop variant. What makes it a desktop app is what
the shell injects before the portal's first script runs, the commands it answers
afterwards, and the events it sends when what it injected has moved.

Closing that window closes the app, and the node goes down with it deliberately:
the close is held back, the supervisor stops the node the way its own shutdown
expects, and the app exits once that is done. Closing it again while the stop is
running changes nothing. [`app_quit`](#commands) is the same path, for a portal
that has a quit of its own to offer.
The native File menu offers **Quit Aruna Desktop** with `Command+Q` on macOS
and `Ctrl+Q` elsewhere, using that same graceful shutdown path.

```
window  ->  the portal artifact in ui/        (one build, web and desktop)
shell   ->  window.__ARUNA_DESKTOP__          (api base, features, bridge)
events  ->  context-changed, navigate, …      (what moved since boot)
bridge  ->  node supervisor, settings, compute probe, enrollment, paths, wipe
node    ->  a supervised aruna child process on loopback
```

## Layout

A plain Rust application: the crate is the repository.

```
Cargo.toml  tauri.conf.json  build.rs      the app
src/                                       its sources, fallback.html included
capabilities/                              what the app window may call
icons/                                     the bundle icon set, from the brand mark
versions.json                              the pinned aruna version and portal release
scripts/sync-frontend.sh                   installs the portal into ui/
scripts/install-dev-desktop.sh             desktop entry + icons for a dev run
ui/                                        the portal artifact (gitignored)
```

The icon set is generated from the portal's own brand mark, padded to a tenth
of the square so it does not sit cramped in a dock:

```bash
npx --yes @tauri-apps/cli@2 icon <padded-mark>.png -o icons
```

## The shell context

The shell defines `window.__ARUNA_DESKTOP__` in a Tauri initialization script,
which the webview runs at document start, so the portal reads it the moment it
boots. On the web the object is absent and the same bundle behaves like any
hosted portal.

```jsonc
{
  "revision": 0,                                  // 0 is the injected one
  "apiBaseUrl": "http://127.0.0.1:34116/api/v1",
  "authCallbackOrigin": "https://aruna.example",  // the realm website's origin
  "features": { "desktop": true, "systemBrowserAuth": false, "embeddedAuth": true },
  "bridge": { "invoke": "…", "version": 8 }
}
```

`version` is the command set below. A shell that answers fewer commands must not
claim version 8.

### Which API the portal talks to

| State | `apiBaseUrl` |
| --- | --- |
| enrolled **and** the node is serving | `http://127.0.0.1:34116/api/v1` |
| otherwise, with a realm connected | `{realm origin}/api/v1` |
| neither | `http://127.0.0.1:34116/api/v1`, the only API this app knows |

**Serving**, not spawned. A node binds its listeners only once its join and its
startup are done, so a freshly started one would refuse the portal's first
request. The shell polls `readyz` on the ops port for as long as the child lives
and only then calls the node ready; losing readiness (a stop, a pause, a
restart, a failure) takes the portal back to the realm. The same probe is one
of [the two signals](#the-two-signals) an enrollment is watched by.

An initialization script cannot be changed after its webview exists, and a
reload would only run the old one again. So the injected object is the **first**
context, not the only one: when the choice above moves (a realm is connected,
an enrollment finishes, a device is wiped, the node starts serving or stops),
the shell emits [`context-changed`](#events) carrying the whole context in the
same shape, and `shell_context` answers the same object on demand. The window is
never replaced and the page is never reloaded; the portal switches its API base
in place and keeps the route the owner is on.

The check runs after every command and every node transition that can move the
choice, one turn later, so the command still answers first and the event always
describes state that is already stored. A context equal to the one the portal
was last told is **not** emitted, so the portal sees one event per move.

`revision` numbers those moves: `0` is what the script injected, and each stored
change adds one. Events are emitted outside the lock that stores them, so two
moves in the same instant can arrive in either order; the portal keeps the
highest revision it has seen and drops anything that is not newer, including a
`shell_context` answer that lost a race with an event.

`authCallbackOrigin` is the origin (scheme, host, port) of the remembered realm
URL, because the realm website's `/auth/callback` is the one redirect URI a
provider registers. Without a remembered realm it is the node's own portal
listener while that serves, and otherwise absent, so the portal falls back to
`aruna://auth/callback`. See [Deep links](#deep-links) for what the app itself
accepts.

## Commands

Contract version 8. Arguments are a single object; answers are JSON. A name the
shell does not implement is rejected with `unknown command: …`, which the portal
reports as a missing bridge half rather than a failed call.

| Command | Arguments | Answers |
| --- | --- | --- |
| `shell_context` | – | `{revision, apiBaseUrl, authCallbackOrigin?, realmUrl?, features}` |
| `node_status` | – | `{state, ready, nodeId, realm, enrolled, enrolling, apiBaseUrl, version, uptimeSeconds, message, detail, realmMismatch, updateRequired}` |
| `node_logs_tail` | `{lines}` | the last lines of the node's output, oldest first |
| `node_settings_get` | – | `{storagePath, compute, s3Enabled, paused}` |
| `node_settings_set` | `{settings}` (any subset) | the stored settings |
| `compute_probe` | – | `{docker: {available, version, socket}, apptainer: {available, version, cgroupOk, cgroupRoot}}` |
| `enroll_apply` | `{secret, seedUrl?, realm?, site?, label?}` | `{nodeId, realm}` |
| `wipe_device` | `{confirm}` | – |
| `pick_directory` | `{title?, startPath?}` | the chosen path, or `null` |
| `reveal_path` | `{path}` | – |
| `open_external` | `{url}` | – |
| `auth_open` | `{url}` | – |
| `oidc_discovery` | `{url}` | the provider's discovery document |
| `oidc_token` | `{endpoint, form}` | the provider's token response |
| `validate_realm` | `{input}` | `{origin, realm, apiVersion, portal, redirectedFrom}` |
| `enroll_invite_last` | – | the uncollected `enroll-invite` payload, or `null` |
| `app_quit` | – | – |

Notes on the ones that do more than they say:

* `shell_context` answers what `window.__ARUNA_DESKTOP__` carries **minus
  `bridge`**, as it stands now rather than as it was at boot. It is the same
  payload `context-changed` sends, so a portal that starts late, missed an event
  or wants to be sure reads it here.
* `node_status` reports `ready` beside `state`, because `running` is reached at
  spawn and a node serves a little later: it is the same flag the `node-status`
  event carries, and the one the shell context follows. `apiBaseUrl` follows it
  too; it is filled only while the node **serves**, never while it is merely
  running, so it is never an address that would refuse the call.
* `updateRequired` (`{local, realm}`) is set while the realm runs another aruna
  version than this app bundles. Every node of a realm runs one version, so the
  node is not started, and a running one is stopped within a minute of the
  realm moving on. The state stays until the app is updated or the device is
  wiped and set up again.
* `node_status` is also where a running enrollment is observed: it probes the
  node, advances the join, and spends the one-time secret once the node serves
  the realm. `enrolling` is that join while it lasts: a secret is armed and the
  realm has not listed this device yet, so it turns off in the same answer
  `enrolled` turns on. `nodeId` stays `null`; a node reports its own id only to
  a caller holding a realm token, which the shell never has, so the portal reads
  it with the owner's own.
* `node_settings_set` stops the node when `paused` turns on and starts it when
  it turns off. `storagePath`, `compute`, and `s3Enabled` are read at startup, so
  changing any of them **restarts** a node that is up; everything else reaches the
  node on its next start. Synced folders are **not** shell settings: they live
  in the node, are bound through its `/device/folders` API and are wiped by
  `/device/wipe`.
* `compute_probe` asks the machine, not the settings: it runs
  `docker version --format '{{.Server.Version}}'` and `apptainer version` with
  a short timeout, so `available` means the daemon answered rather than that a
  client is installed. The socket is looked for in the usual places (rootless,
  `/var/run`, Docker Desktop, Colima, OrbStack), the search path gains the
  common install directories, and the socket found is passed to the node as
  `DOCKER_HOST`. A backend that is not installed is unavailable, never an
  error. `cgroupOk` reports whether this login session owns a delegated cgroup
  slice with the `memory` and `pids` controllers. The node refuses to build the
  apptainer backend without one, so the shell does not choose apptainer while
  `cgroupOk` is false.
* `reveal_path` shows a path in the owner's file manager. It must be absolute
  and must exist; a relative one would resolve against the app's own directory,
  and there is nothing to show for a path that is not there. Where it points is
  not restricted: revealing opens a window, reads nothing, and the owner's
  folders live wherever they put them.
* `enroll_apply` needs only the secret. The seed URL and the realm travel inside
  it; the shell keeps its own copy of both only to watch the join. It clears a
  pause, because enrolling is an explicit "run this node".
* `wipe_device` re-checks `confirm` against `wipe`, stops the node, deletes the
  data directory the shell provisioned (and a storage directory outside it),
  clears the enrollment and the storage override, and keeps the machine
  configuration **and the remembered realm**; a wiped device still needs a
  portal to sign in to, and joining another realm starts there. A path that is
  not deep enough to be one of ours is refused rather than deleted. The node's
  own REST wipe is a separate surface, and it is the one that drops the
  device's synced folders.
* `validate_realm` checks the address, **remembers it** as this device's realm
  and answers with the origin. Remembering is the point: it is what the portal
  talks to until a node of this device's own is serving. Only a management node
  enrolls a device or signs it in, so an address answered by any other kind is
  **followed**: the node's `/system/realm` names the realm's management nodes and
  the first one that answers is remembered instead, with the typed address in
  `redirectedFrom`. A node that names none reachable is kept as typed. An **enrolled** device
  refuses it outright with `wipe this device before joining another realm`: its
  node, its keys and its data belong to the realm it joined. A wipe keeps the
  remembered realm, so the owner still has one to sign in to afterwards, and it
  is a realm this device is no longer in.
* `open_external` opens `http` and `https` in the system browser and refuses
  every other scheme.
* `auth_open` opens an `http` or `https` sign-in page in a window of the app's
  own, labelled `login`, and refuses every other scheme. See [Sign-in](#sign-in)
  for how that window comes back.
* `oidc_discovery` and `oidc_token` make the two provider calls the portal
  cannot make itself: no provider accepts the app window's origin for
  cross-origin requests. `oidc_discovery` fetches an `https` URL, or `http` on a
  loopback host, whose path ends with `/.well-known/openid-configuration`, and
  remembers the `token_endpoint` it names: an `https` one off this machine, or
  a loopback one on the discovery URL's own origin.
  `oidc_token` posts `form` as `application/x-www-form-urlencoded`, and only to
  an endpoint remembered that way in this run, so the shell is no general proxy.
  Neither follows redirects, both give up after 15 seconds, and the form and
  the answer are never logged.
  A refused token request fails with
  `the provider refused the token request (HTTP {status}): {reason}`, the reason
  being the provider's `error_description` or `error`.
* `enroll_invite_last` answers with the last `enroll-invite` payload the shell
  emitted, in exactly the shape it was emitted, and never carries the secret. It
  is how a portal that subscribed after a cold-start link still sees it, so it
  serves that payload **once**: collecting it clears it, and a later visit gets
  `null` rather than replaying an old outcome. A wipe clears it too.
* `app_quit` ends the app, and stops the node first: `SIGTERM`, then the node's
  own shutdown grace, then a kill. It is the same path closing the window takes,
  and the portal's only way to close a window it does not own. Whichever comes
  first owns the shutdown; a second one waits for it rather than stopping the
  node again.

## Events

Five events reach the app window, and none of them is a source of truth: the
first three have a command answering the same thing, the fourth is a deep link
the owner followed, and the fifth is a sign-in window the owner closed.

| Event | Payload | Emitted |
| --- | --- | --- |
| `context-changed` | the shell context, as injected minus `bridge` | whenever the context moves, once per move, never unchanged |
| `node-status` | `{state, ready, pid, binary, detail, restarts, lastExit}` | on every supervisor state change |
| `enroll-invite` | [see below](#enroll-invite) | once an `aruna://enroll` link has been acted on |
| `navigate` | `{path}` | when a deep link belongs in the portal's router |
| `auth-cancelled` | – | when the `login` window went away before the provider answered |

`context-changed` is emitted after `validate_realm`, after `node_settings_set`,
after `enroll_apply` and an armed `aruna://enroll`, after `wipe_device`, while
`node_status` settles a finishing enrollment, and on every node transition into
or out of **serving**: the node answering, a stop, a pause, a restart, or a
failure. Each of those stores its own state first, so the context the event
carries is the one `shell_context` answers with.

```jsonc
{
  "revision": 3,
  "apiBaseUrl": "https://aruna.example/api/v1",
  "realmUrl": "https://aruna.example",
  "features": { "desktop": true, "systemBrowserAuth": false, "embeddedAuth": true }
}
```

`navigate` carries a route of the portal's own router, with the query and the
fragment the link arrived with, and the window is brought forward with it:

```jsonc
{ "path": "/auth/callback?code=abc&state=xyz" }
```

It is an event rather than a navigation because loading that URL would reload
the page and throw away the session and the context the portal has built. The
cost is a **cold start**: a link that launches the app can route before the
portal is listening. An `aruna://enroll` link is covered (its outcome waits in
`enroll_invite_last`, and the device page reads the same enrollment out of
`node_status`), while an `aruna://auth/callback` into a cold start is out of
scope: a sign-in this app started has a portal listening by definition.

## The node binary

The node the app supervises is the Tauri sidecar `binaries/aruna-<target
triple>`: the aruna workspace's own release binary, nothing forked. Its
version is pinned in `versions.json` and is also the version of this app. The
sidecar is gitignored and installed with:

```bash
scripts/sync-node.sh                  # builds ../aruna (or $ARUNA_DIR) in release mode
scripts/sync-node.sh --no-build       # copies the binary that is already built
ARUNA_NODE_BIN=/path/to/aruna scripts/sync-node.sh
```

The build is `--locked` and stripped. Its features are `docker,apptainer` on
Linux and `docker` elsewhere, because apptainer needs Linux-only APIs;
`kubernetes` is never built in. The script refuses a checkout whose version is
not the pinned one.

tauri-build copies it next to the app executable, so a `cargo run` finds it in
`target/<profile>/aruna`, and packaged builds bundle exactly that file: one
installer carries the app and its node. The lookup order stays
`ARUNA_DESKTOP_NODE_BIN`, the `binaryPath` setting, the sidecar next to the
executable, then the resource directory. Without a synced node the build still
succeeds: `build.rs` writes a placeholder in the sidecar's place, and the app
reports `run scripts/sync-node.sh` instead of running it.

## Releases

`.github/workflows/release.yml` builds one installer per platform, each with
the app, the node and the portal inside: `.deb`, `.rpm` and `.AppImage` for
Linux (built on Ubuntu 22.04 for an older glibc), an NSIS `.exe` for Windows
(MSI rejects pre-release versions), and a `.dmg` for Apple silicon. It checks
out `arunaengine/aruna` at the ref pinned in `versions.json`, builds the node
for the target, installs it and the pinned portal release through the two sync
scripts, and runs `tauri build`. A `v*` tag must be `v` plus the pinned aruna
version and publishes the bundles as a GitHub release; a manual run only
uploads them as workflow artifacts.

## The portal artifact

`ui/` holds the portal build the window serves. It is gitignored, and it is the
same artifact a node serves. The release is pinned in `versions.json` and is
downloaded and checked against its sha256:

```bash
scripts/sync-frontend.sh
```

If the download fails, the pinned website ref is exported from `../website` (or
`$ARUNA_WEBSITE_DIR`) and built in a scratch directory. The checkout itself is
not touched.

The directory is called `ui` rather than `dist` because `dist` would read as
this repository's own build output, which it is not: it is someone else's build,
vendored in. Packaged builds bundle exactly what that script installs.

Without it, the build writes a placeholder page naming the command
(`src/fallback.html`, installed by `build.rs`), so a checkout that never synced
a portal still builds and still explains itself.

### Development

Linux needs the Tauri v2 prerequisites: `webkit2gtk-4.1`, `gtk3`,
`libayatana-appindicator`, `librsvg`, and `patchelf`. This repository has no npm
tooling of its own.

Two ways to run it:

```bash
# 1. against the website's dev server, with hot reload
scripts/sync-node.sh
(cd ../website && npx vite --port 1420 --strictPort)
cargo run

# 2. against the installed artifact, as a packaged build serves it
scripts/sync-node.sh
scripts/sync-frontend.sh
cargo run --release --features custom-protocol
```

Both find the node through the sidecar `scripts/sync-node.sh` installs;
`ARUNA_DESKTOP_NODE_BIN=/path/to/aruna` still overrides it for one run.

The `custom-protocol` feature is what embeds `ui/`; without it Tauri loads the
dev URL, whatever the profile says. The Tauri CLI turns it on for `tauri build`
by itself.

On Wayland the app sets `__NV_DISABLE_EXPLICIT_SYNC=1` before the webview starts
when the NVIDIA driver is loaded, and `WEBKIT_DISABLE_DMABUF_RENDERER=1` on other
GPUs. It sets neither when one of them or `ARUNA_DESKTOP_NO_WAYLAND_WORKAROUND` is
already set.

### The window icon on Linux

A compositor does not read the icon out of the running app: it looks for an
installed desktop entry whose name matches the window's app id. Packaged builds
install one, so their windows carry the Aruna mark; a `cargo run` installs
nothing, and the window shows a placeholder. Once per machine:

```bash
scripts/install-dev-desktop.sh            # --remove undoes it
```

It installs `icons/` into `~/.local/share/icons/hicolor/<size>/apps/` and writes
`~/.local/share/applications/<app id>.desktop`, then refreshes the desktop and
icon caches where those tools exist.

The app id is **`aruna-desktop`**, the binary name, not the bundle identifier:
GTK takes a window's Wayland `app_id` from the program name, which a window of
this app confirms with `set_app_id("aruna-desktop")` under `WAYLAND_DEBUG=1`.
The script derives it the same way rather than assuming, and honours
`app.enableGtkAppId` if that is ever switched on.

It does not claim the `aruna` scheme: the deep-link plugin writes its own
`aruna-desktop-handler.desktop` for that at runtime, and the two entries would
otherwise fight over it.

Checks:

```bash
cargo check --all-targets
cargo test
cargo +nightly fmt --all -- --check
cargo +nightly clippy --all-targets -- -D warnings
```

Bundling goes through the Tauri CLI, which this repository does not vendor:

```bash
npx --yes @tauri-apps/cli@2 build      # or: tauri info, tauri dev
```

The CLI finds the app by looking for `tauri.conf.json`, and finds it at the
repository root, so no `--config` flag and no `src-tauri` shim is needed.

## Sidecar model

The node is a **child process**, not a library. `src/node/` resolves a
node binary, writes its configuration, starts it, tails its output, restarts it
after an unexpected exit, and stops it with `SIGTERM` followed by a kill once
the node's own shutdown grace has passed.

Binary resolution, most specific first:

1. `ARUNA_DESKTOP_NODE_BIN` (development override),
2. the path stored in the settings file,
3. `aruna` next to the desktop executable (the bundled sidecar),
4. `<resources>/binaries/aruna`.

Release bundles include the sidecar. A development checkout without a synced
node uses a recognised placeholder and reports how to install the binary.

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
| `S3_ADDRESS`, `S3_HOST` | `127.0.0.1:34118` by default, **empty** once the owner turns the listener off |
| `S3_PUBLIC_URL` | `http://127.0.0.1:34118`, while the local S3 endpoint is on |
| `ONBOARDING_SECRET` | the one-time enrollment secret, consumed on the next start |
| `ARUNA_COMPUTE_EXECUTOR` | the resolved backend, or `off`; the rest of the profile follows below |

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

* `S3_HOST` and `S3_ADDRESS` carry the loopback endpoint by default, so the owner
  has an S3 endpoint of their own on this machine. Turning it off with
  `"s3Enabled": false` in `<app config>/settings.json` writes both **empty**,
  which is how the node is told to serve no S3 listener. They are written empty
  rather than omitted, because an omitted key leaves an inherited or
  `.env`-provided one in place. Setting only one of the pair aborts the node, so
  they always move together.
* `PORTAL_MODE` is written as `disabled` when no portal directory is set. It is
  never written empty: aruna accepts only `artifact` and `disabled`.

### Local compute

The owner may run their own jobs on this machine (**never** the realm's: the
node is told so explicitly). Before every node start the shell probes the
machine, resolves the owner's `backend` choice against what answered, and writes
the profile below. `ARUNA_COMPUTE_EXECUTOR` is **always** written (`off` when
nothing was resolved, which is `off` itself or a machine with no usable backend)
and the rest of the profile only while an executor was resolved. The
supervisor also strips every inherited `ARUNA_COMPUTE_*` key from the child, so
the environment the app was launched from cannot configure the node's compute.

| Key | Value written by the app |
| --- | --- |
| `ARUNA_COMPUTE_EXECUTOR` | `docker` or `apptainer`, the resolved backend, or `off` |
| `ARUNA_COMPUTE_OPTIONAL` | `1`; a missing daemon leaves the node up without compute |
| `ARUNA_COMPUTE_LOCAL_ONLY` | `1`; the device's S3 listener is loopback-only, so there is none to hand a container |
| `ARUNA_COMPUTE_MAX_CONCURRENT` | the owner's concurrent-run ceiling, `1` by default; omitted when they unset it |
| `ARUNA_COMPUTE_MAX_CPU_CORES`, `ARUNA_COMPUTE_MAX_RAM_BYTES`, `ARUNA_COMPUTE_MAX_DISK_BYTES` | each written only while the owner set it; unset means no ceiling |
| `ARUNA_COMPUTE_KEEP_FAILED` | `1` while the owner keeps failed jobs for inspection |
| `ARUNA_COMPUTE_APPTAINER_CGROUP_ROOT` | the delegated user slice; apptainer is only ever chosen with one |
| `ARUNA_COMPUTE_STATE_ROOT` | `<app data>/node/compute` |
| `ARUNA_COMPUTE_APPTAINER_STATE_ROOT`, `ARUNA_COMPUTE_APPTAINER_SIF_CACHE` | `…/compute/apptainer` and `…/compute/sif` (apptainer only) |

These key names **must** match the node's configuration and compute wiring in
the Aruna repository (`aruna/src/config.rs` and `aruna/src/main.rs`).
`ARUNA_COMPUTE_LOCAL_ONLY`, `ARUNA_COMPUTE_KEEP_FAILED`, and
`ARUNA_COMPUTE_STATE_ROOT` are read by the current node wiring.

`auto` prefers docker, because it needs no delegated cgroup of ours, and falls
back to apptainer **only** where a delegated user slice was detected. Apptainer
the owner asked for is dropped the same way when no slice was found, with a line
in the node's log saying so: without one the node exits instead of starting.
A chosen `docker` is written whatever the probe found, so the node reports the
real reason it could not build one instead of the shell picking another.

## Settings the owner sees

`node_settings_get` and `node_settings_set` speak this shape, and it is the
whole local control surface:

```jsonc
{
  "storagePath": "/home/owner/.local/share/engine.aruna.desktop/node/storage",
  "compute": {
    "backend": "auto",          // auto | docker | apptainer | off
    "maxConcurrent": 1,         // null here too: as many as the node will run
    "maxCpuCores": null,        // null is no ceiling, not a default
    "maxRamBytes": null,
    "maxDiskBytes": null,
    "keepFailed": false
  },
  "s3Enabled": true,
  "paused": false
}
```

A patch may carry any subset of the top level, but `compute` is replaced whole:
an absent ceiling means "no ceiling", which a per-field merge could not tell
from "unchanged". Zero is refused for every compute number; `null` is how the
owner says "no limit".

The starting values are deliberately modest rather than a policy: one job at a
time, no ceilings, the machine's own backend. It is the owner's machine.

An enrolled device, or one with an armed enrollment secret, starts its node
when the app launches unless paused. There is no `autoStart` setting.

**Paused means nothing runs.** A paused node is stopped, so it serves nothing,
syncs nothing and computes nothing; local jobs are refused with it. Resuming
starts it again. The shell passes no pause state to the node, because there is
no node to pass it to.

## Deep links

The `aruna` scheme is registered by the app (`tauri-plugin-deep-link`), and
`tauri-plugin-single-instance` forwards a second launch to the running window.
Both accept the authority-less form (`aruna:enroll?…`). Unknown routes are
dropped with a warning that names the route only, never the query.

| Link | Behaviour |
| --- | --- |
| `aruna://enroll?secret=…&seed=…&realm=…&site=…` | the enrollment is armed, `navigate` asks for `/app/device`, and `enroll-invite` is emitted |
| `aruna://auth/callback?…` | `navigate` asks for `/auth/callback` with the query **and** the fragment the provider returned |

`secret` and `seed` are required in a link, and `seed` must be an `http`/`https`
origin; a link missing either is dropped. The optional `site` is the origin of
the realm website that showed the link. It is remembered as the realm URL, so
it becomes `authCallbackOrigin`, while the seed's `/api/v1` stays the API the
portal talks to; a `site` that is no web origin is ignored. `enroll_apply`
takes the same `site` for a pasted link. The owner already accepted the
invitation by following it out of the portal, so the shell arms it rather than
stashing it for a second confirmation.

### `enroll-invite`

```jsonc
{ "seed": "https://seed.example", "realm": "r-1", "applied": true, "error": null }
```

The secret is never part of it. A link followed into a cold start can deliver
the event before the portal is listening, so the same payload waits behind
`enroll_invite_last` for one collection, and the [`navigate`](#events) event
asks for `/app/device`, where `node_status` reports the same enrollment either
way.

## Enrollment

The seed mints the secret and the deep link; `aruna/api/src/routes/onboarding.rs`
builds exactly the three query keys above.

The node needs **only `ONBOARDING_SECRET`**. The seed URL, the realm id, the
enrollment id and the onboarding mode all travel inside the secret itself
(`core/src/onboarding.rs`, postcard plus URL-safe base64), which the joiner
decodes to reach `{seed}/api/v1/access/onboarding/bootstrap`.

The secret is stored once, is never sent to the webview, never logged, and is
cleared the moment the node serves the realm. It must not be cleared earlier: a
node whose join is half done needs it again to resume.

### The two signals

The device cannot poll `GET {seed}/api/v1/access/onboarding/secrets/{id}/status`: the
deep link carries no enrollment id, and the route needs a realm bearer token.
Enrollment progress is therefore observed, never queried:

1. `GET http://127.0.0.1:34117/readyz` turning `200`. The node binds its
   listeners only after the join has succeeded, and nothing is logged when it
   does, so readiness is the authoritative "the seed accepted this device". The
   supervisor polls it on every start for its own reason too: it is what makes
   the node [the API the portal talks to](#which-api-the-portal-talks-to).
2. `GET http://127.0.0.1:34116/api/v1/system/info` reporting the `realm_id` from the
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

Sign-in stays inside the app (`embeddedAuth`): the portal hands the
authorization URL to `auth_open`, which opens the provider's page in a window
of the app's own. The provider redirects that window to the realm website's
`/auth/callback` under `authCallbackOrigin`, and the website moves a desktop
sign-in on to `aruna://auth/callback` with the same query. That navigation is
intercepted before it loads, whether a redirect or a page script starts it,
routed exactly like the deep link and landed on the portal's callback route in
the app window, and the sign-in window closes on it. A sign-in window the owner
closes instead emits `auth-cancelled`, so the portal drops the sign-in it was
waiting on.

An `aruna://auth/callback` link that arrives from the outside is still
accepted. The `systemBrowserAuth` flag is what the shell switches off for this;
a portal that reads neither flag falls back to navigating its own window.

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
