<script setup lang="ts">
import { ref, watch } from 'vue'
import * as api from '../lib/api'
import { pending, refresh, report } from '../lib/store'

const secret = ref('')
const reveal = ref(false)
const busy = ref(false)
const done = ref(false)

// A deep link arrives at any time, so its secret fills the form when it lands.
watch(
  pending,
  (payload) => {
    if (payload?.secret) {
      secret.value = payload.secret
    }
  },
  { immediate: true },
)

async function apply() {
  busy.value = true
  done.value = false
  try {
    await api.enrollApply(secret.value)
    await refresh()
    done.value = true
  } catch (error) {
    report(error)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section v-if="pending" class="panel">
    <h2>Invitation received</h2>
    <dl class="grid">
      <dt>Realm</dt>
      <dd>{{ pending.realm ?? 'unnamed' }}</dd>
      <dt>Device</dt>
      <dd>{{ pending.name ?? 'this device' }}</dd>
      <dt>Endpoints</dt>
      <dd>{{ pending.endpoints.length > 0 ? pending.endpoints.join(', ') : 'from the realm document' }}</dd>
      <dt>Secret</dt>
      <dd class="mono">{{ reveal ? pending.secret : '••••••••••••' }}</dd>
    </dl>
    <label class="check">
      <input v-model="reveal" type="checkbox" />
      Show the secret
    </label>
  </section>

  <section class="panel">
    <h2>Enroll this device</h2>
    <p class="muted">
      Paste the one-time secret from the portal wizard, or open its
      <code>aruna://enroll</code> link. The node consumes the secret on its next
      start, so applying it restarts the node.
    </p>
    <textarea v-model="secret" rows="3" spellcheck="false" placeholder="one-time secret"></textarea>
    <div class="row">
      <button :disabled="busy || secret.trim().length === 0" @click="apply">
        Apply and restart
      </button>
      <span v-if="done" class="muted">Stored. Watch the status view for the outcome.</span>
    </div>
  </section>
</template>
