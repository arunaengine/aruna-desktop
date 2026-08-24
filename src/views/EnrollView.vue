<script setup lang="ts">
import { computed, ref } from 'vue'
import * as api from '../lib/api'
import { advance, invite, progress, report } from '../lib/store'

const text = ref('')
const seed = ref('')
const realm = ref('')
const busy = ref(false)

// A pasted deep link carries its own seed, so the field is only needed beside
// a bare secret.
const pastedLink = computed(() => text.value.trim().toLowerCase().startsWith('aruna:'))

async function run(action: () => Promise<unknown>) {
  busy.value = true
  try {
    await action()
    await advance()
  } catch (error) {
    report(error)
  } finally {
    busy.value = false
  }
}

const accept = () => run(api.enrollAccept)
const paste = () => run(() => api.enrollPaste(text.value, seed.value, realm.value))
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

  <section class="panel">
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
      <span v-if="progress.phase === 'failed'" class="muted">{{ progress.detail }}</span>
    </div>
  </section>
</template>
