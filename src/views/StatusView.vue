<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import * as api from '../lib/api'
import { refresh, report, status } from '../lib/store'
import type { HealthReport, LogLine, NodeState } from '../lib/types'

const POLL_MS = 1500
const KEEP_LINES = 400

const health = ref<HealthReport | null>(null)
const lines = ref<LogLine[]>([])
const cursor = ref(0)
const busy = ref(false)
let timer: number | undefined

const labels: Record<NodeState, string> = {
  stopped: 'Stopped',
  starting: 'Starting',
  running: 'Running',
  stopping: 'Stopping',
  restarting: 'Restarting',
  failed: 'Failed',
  binaryMissing: 'No node binary',
}

const label = computed(() => labels[status.value.state])
const live = computed(() => status.value.state === 'running')

async function poll() {
  try {
    const page = await api.nodeLogs(cursor.value)
    cursor.value = page.cursor
    if (page.lines.length > 0) {
      lines.value = [...lines.value, ...page.lines].slice(-KEEP_LINES)
    }
    health.value = live.value ? await api.nodeHealth() : null
  } catch (error) {
    report(error)
  }
}

async function act(action: () => Promise<unknown>) {
  busy.value = true
  try {
    await action()
    await refresh()
  } catch (error) {
    report(error)
  } finally {
    busy.value = false
  }
}

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
  <section class="panel">
    <div class="row">
      <span class="pill" :data-state="status.state">{{ label }}</span>
      <span v-if="status.pid" class="muted">pid {{ status.pid }}</span>
      <span v-if="status.restarts > 0" class="muted">{{ status.restarts }} restarts</span>
    </div>
    <p v-if="status.binary" class="muted mono">{{ status.binary }}</p>
    <p v-if="status.detail" class="detail">{{ status.detail }}</p>
    <div class="row">
      <button :disabled="busy" @click="act(api.nodeStart)">Start</button>
      <button :disabled="busy" @click="act(api.nodeStop)">Stop</button>
      <button :disabled="busy" @click="act(api.nodeRestart)">Restart</button>
      <button :disabled="!live" @click="act(api.portalOpen)">Open portal</button>
    </div>
  </section>

  <section class="panel">
    <h2>Health</h2>
    <p v-if="!health" class="muted">The ops endpoint is polled while the node runs.</p>
    <dl v-else class="grid">
      <dt>/healthz</dt>
      <dd>{{ health.health.ok ? 'ok' : health.health.error ?? health.health.status }}</dd>
      <dt>/readyz</dt>
      <dd>{{ health.ready.ok ? 'ready' : health.ready.error ?? health.ready.status }}</dd>
    </dl>
    <pre v-if="health?.ready.body" class="body">{{ health.ready.body }}</pre>
  </section>

  <section class="panel">
    <h2>Log</h2>
    <p v-if="lines.length === 0" class="muted">No output yet.</p>
    <pre v-else class="log"><span
      v-for="line in lines"
      :key="line.seq"
      :class="line.stream"
    >{{ line.text }}
</span></pre>
  </section>
</template>
