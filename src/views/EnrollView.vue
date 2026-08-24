<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import * as api from '../lib/api'
import { advance, invite, openPortal, progress, report, syncRealm, tab } from '../lib/store'
import type { EnrollPhase, LogLine } from '../lib/types'

const POLL_MS = 1500
const KEEP_LINES = 200

const text = ref('')
const seed = ref('')
const realm = ref('')
const busy = ref(false)
const lines = ref<LogLine[]>([])
const cursor = ref(0)
const again = ref(false)
let timer: number | undefined

// A pasted deep link carries its own seed, so the field is only needed beside
// a bare secret.
const pastedLink = computed(() => text.value.trim().toLowerCase().startsWith('aruna:'))

const steps: Array<{ phase: EnrollPhase; label: string; hint: string }> = [
  {
    phase: 'received',
    label: 'Invitation accepted',
    hint: 'The one-time secret is stored for the next start.',
  },
  {
    phase: 'starting',
    label: 'Node restarting',
    hint: 'The node redeems the secret against the seed while it boots.',
  },
  {
    phase: 'waiting',
    label: 'Seed accepted the device',
    hint: 'The node is ready, so the seed granted it a place in the realm.',
  },
  {
    phase: 'visible',
    label: 'Serving the realm',
    hint: 'The node answers for the realm the invitation named.',
  },
]

const rank: Record<EnrollPhase, number> = {
  idle: -1,
  received: 0,
  starting: 1,
  waiting: 2,
  visible: 3,
  done: 4,
  failed: -1,
}

const done = computed(() => progress.value.phase === 'done')
const failed = computed(() => progress.value.phase === 'failed')
const running = computed(() => progress.value.phase !== 'idle')
const reached = computed(() => rank[progress.value.phase])

function mark(index: number): string {
  if (failed.value) {
    return 'pending'
  }
  if (index < reached.value) {
    return 'done'
  }
  return index === reached.value ? 'current' : 'pending'
}

async function poll() {
  try {
    const page = await api.nodeLogs(cursor.value)
    cursor.value = page.cursor
    if (page.lines.length > 0) {
      lines.value = [...lines.value, ...page.lines].slice(-KEEP_LINES)
    }
    await advance()
  } catch (error) {
    report(error)
  }
}

async function run(action: () => Promise<unknown>) {
  busy.value = true
  lines.value = []
  cursor.value = 0
  try {
    await action()
    await advance()
    await syncRealm()
  } catch (error) {
    report(error)
  } finally {
    busy.value = false
  }
}

const accept = () => run(api.enrollAccept)
const paste = () => run(() => api.enrollPaste(text.value, seed.value, realm.value))

onMounted(() => {
  void poll()
  timer = window.setInterval(poll, POLL_MS)
})

onUnmounted(() => {
  if (timer !== undefined) {
    window.clearInterval(timer)
  }
})
</script>

<template>
  <section v-if="invite" class="panel">
    <h2>Invitation received</h2>
    <dl class="grid">
      <dt>Realm</dt>
      <dd>{{ invite.realm ?? 'named by the seed' }}</dd>
      <dt>Seed</dt>
      <dd class="mono">{{ invite.seed }}</dd>
    </dl>
    <p class="muted">
      The one-time secret stays in the app. Enrolling restarts the node, which
      redeems the secret against the seed while it boots.
    </p>
    <div class="row">
      <button :disabled="busy" @click="accept">Enroll this device</button>
    </div>
  </section>

  <section v-if="done" class="panel">
    <h2>Enrolled</h2>
    <p>
      This device belongs to <strong>{{ progress.realm ?? 'the realm' }}</strong>.
      The one-time secret is spent and has been cleared.
    </p>
    <div class="row">
      <button @click="openPortal()">Open the realm portal</button>
      <button @click="tab = 'status'">Go to status</button>
      <button class="link" @click="again = true">Enroll again</button>
    </div>
  </section>

  <section v-if="running && !done" class="panel">
    <h2>Enrollment</h2>
    <p v-if="failed" class="failure">{{ progress.detail ?? 'the enrollment failed' }}</p>
    <ol class="steps">
      <li v-for="(step, index) in steps" :key="step.phase" :data-mark="mark(index)">
        <span class="label">{{ step.label }}</span>
        <span class="muted">{{ step.hint }}</span>
      </li>
    </ol>
    <dl class="grid">
      <dt>Realm</dt>
      <dd>{{ progress.realm ?? 'not known yet' }}</dd>
      <dt>Seed</dt>
      <dd class="mono">{{ progress.seed ?? 'none' }}</dd>
    </dl>
    <p v-if="failed" class="muted">
      Ask the portal wizard for a fresh invitation and enroll again.
    </p>
  </section>

  <section v-if="running && !done" class="panel">
    <h2>Node output</h2>
    <p v-if="lines.length === 0" class="muted">No output yet.</p>
    <pre v-else class="log"><span
      v-for="line in lines"
      :key="line.seq"
      :class="line.stream"
    >{{ line.text }}
</span></pre>
  </section>

  <section v-if="!done || again" class="panel">
    <h2>Paste an invitation</h2>
    <p class="muted">
      Paste the whole <code>aruna://enroll</code> link from the portal wizard, or
      the bare secret together with the seed's base URL.
    </p>
    <textarea
      v-model="text"
      rows="3"
      spellcheck="false"
      placeholder="aruna://enroll?secret=… or the bare secret"
    ></textarea>
    <template v-if="!pastedLink">
      <label>
        Seed URL
        <input v-model="seed" spellcheck="false" placeholder="https://seed.example" />
      </label>
      <label>
        Realm (optional)
        <input v-model="realm" spellcheck="false" placeholder="asked of the seed" />
      </label>
    </template>
    <div class="row">
      <button :disabled="busy || text.trim().length === 0" @click="paste">
        Enroll this device
      </button>
    </div>
  </section>
</template>
