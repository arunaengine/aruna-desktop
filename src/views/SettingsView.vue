<script setup lang="ts">
import { onMounted, ref } from 'vue'
import * as api from '../lib/api'
import { realm, report, tab } from '../lib/store'
import type { NodeSettings, Paths } from '../lib/types'

interface Form {
  binaryPath: string
  storagePath: string
  portalDir: string
  portalPort: number
  apiPort: number
  opsPort: number
  s3Port: number
  p2pPort: number
  s3Enabled: boolean
  graceSecs: number
  autoStart: boolean
  preferLocalPortal: boolean
}

const form = ref<Form | null>(null)
const paths = ref<Paths | null>(null)
const saved = ref(false)
let current: NodeSettings | null = null

function fill(settings: NodeSettings): Form {
  return {
    binaryPath: settings.binaryPath ?? '',
    storagePath: settings.storagePath ?? '',
    portalDir: settings.portalDir ?? '',
    portalPort: settings.ports.portal,
    apiPort: settings.ports.api,
    opsPort: settings.ports.ops,
    s3Port: settings.ports.s3,
    p2pPort: settings.ports.p2p,
    s3Enabled: settings.s3Enabled,
    graceSecs: settings.shutdownGraceSecs,
    autoStart: settings.autoStart,
    preferLocalPortal: settings.preferLocalPortal,
  }
}

function trimmed(value: string): string | null {
  const text = value.trim()
  return text.length > 0 ? text : null
}

function drain(base: NodeSettings, edited: Form): NodeSettings {
  return {
    ...base,
    binaryPath: trimmed(edited.binaryPath),
    storagePath: trimmed(edited.storagePath),
    portalDir: trimmed(edited.portalDir),
    ports: {
      portal: Number(edited.portalPort),
      api: Number(edited.apiPort),
      ops: Number(edited.opsPort),
      s3: Number(edited.s3Port),
      p2p: Number(edited.p2pPort),
    },
    s3Enabled: edited.s3Enabled,
    shutdownGraceSecs: Number(edited.graceSecs),
    autoStart: edited.autoStart,
    preferLocalPortal: edited.preferLocalPortal,
  }
}

async function save() {
  if (!form.value || !current) {
    return
  }
  saved.value = false
  try {
    current = await api.settingsSave(drain(current, form.value))
    paths.value = await api.pathsGet()
    saved.value = true
  } catch (error) {
    report(error)
  }
}

onMounted(async () => {
  try {
    current = await api.settingsGet()
    form.value = fill(current)
    paths.value = await api.pathsGet()
  } catch (error) {
    report(error)
  }
})
</script>

<template>
  <section v-if="form" class="panel">
    <h2>Node binary</h2>
    <label>
      Path
      <input v-model="form.binaryPath" placeholder="bundled sidecar" spellcheck="false" />
    </label>
    <p class="muted">
      Left empty, the bundled sidecar is used. <code>ARUNA_DESKTOP_NODE_BIN</code>
      overrides both while developing.
    </p>
  </section>

  <section v-if="form" class="panel">
    <h2>Storage</h2>
    <label>
      Storage path
      <input v-model="form.storagePath" placeholder="app data directory" spellcheck="false" />
    </label>
    <label>
      Portal directory
      <input v-model="form.portalDir" placeholder="portal listener disabled" spellcheck="false" />
    </label>
  </section>

  <section v-if="form" class="panel">
    <h2>Portal</h2>
    <dl class="grid">
      <dt>Realm</dt>
      <dd class="mono">{{ realm ?? 'none connected yet' }}</dd>
    </dl>
    <label class="check">
      <input v-model="form.preferLocalPortal" type="checkbox" />
      Open the local portal (offline) while this node serves one
    </label>
    <div class="row">
      <button class="link" @click="tab = 'welcome'">Change realm</button>
    </div>
  </section>

  <section v-if="form" class="panel">
    <h2>Loopback ports</h2>
    <div class="ports">
      <label>Portal<input v-model.number="form.portalPort" type="number" /></label>
      <label>API<input v-model.number="form.apiPort" type="number" /></label>
      <label>Ops<input v-model.number="form.opsPort" type="number" /></label>
      <label>S3<input v-model.number="form.s3Port" type="number" /></label>
      <label>P2P<input v-model.number="form.p2pPort" type="number" /></label>
    </div>
    <label class="check">
      <input v-model="form.s3Enabled" type="checkbox" />
      Serve the local S3 endpoint
    </label>
    <label class="check">
      <input v-model="form.autoStart" type="checkbox" />
      Start the node when the app opens
    </label>
    <label>
      Shutdown grace (seconds)
      <input v-model.number="form.graceSecs" type="number" min="16" />
    </label>
  </section>

  <section class="panel">
    <div class="row">
      <button @click="save">Save</button>
      <span v-if="saved" class="muted">Saved. Restart the node to apply.</span>
    </div>
  </section>

  <section v-if="paths" class="panel">
    <h2>Locations</h2>
    <dl class="grid mono">
      <dt>Settings</dt>
      <dd>{{ paths.configFile }}</dd>
      <dt>Node data</dt>
      <dd>{{ paths.dataDir }}</dd>
      <dt>Storage</dt>
      <dd>{{ paths.storageDir }}</dd>
      <dt>Env file</dt>
      <dd>{{ paths.envFile }}</dd>
    </dl>
  </section>
</template>
