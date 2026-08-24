<script setup lang="ts">
import { ref } from 'vue'
import * as api from '../lib/api'
import { realm, tab } from '../lib/store'
import type { RealmTarget } from '../lib/types'

const address = ref(realm.value ?? '')
const busy = ref(false)
const failure = ref('')
const target = ref<RealmTarget | null>(null)

async function connect() {
  busy.value = true
  failure.value = ''
  target.value = null
  try {
    const found = await api.realmConnect(address.value)
    target.value = found
    realm.value = found.url
    address.value = found.url
  } catch (error) {
    failure.value = error instanceof Error ? error.message : String(error)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="panel hero">
    <h2>Connect to your realm</h2>
    <p>
      Enter the address of the realm you belong to. The app checks it, remembers
      it, and opens its portal in a second window where you sign in as usual.
    </p>
    <label>
      Realm address
      <input
        v-model="address"
        spellcheck="false"
        autocapitalize="off"
        placeholder="aruna.example or https://aruna.example"
        @keyup.enter="connect"
      />
    </label>
    <p class="muted">
      A bare host name is read as <code>https</code>, and a pasted portal link is
      reduced to its origin.
    </p>
    <div class="row">
      <button :disabled="busy || address.trim().length === 0" @click="connect">
        {{ busy ? 'Checking…' : 'Connect' }}
      </button>
      <button class="link" @click="tab = 'enroll'">I have an enroll code</button>
    </div>
    <p v-if="failure" class="failure">{{ failure }}</p>
    <template v-if="target">
      <p>
        Connected to <strong>{{ target.url }}</strong
        >. The portal is open in its own window.
      </p>
      <dl class="grid">
        <dt>Realm</dt>
        <dd>{{ target.realm ?? 'not reported' }}</dd>
        <dt>API version</dt>
        <dd>{{ target.apiVersion ?? 'not reported' }}</dd>
      </dl>
      <p v-if="!target.portal" class="detail">
        This address answers as an Aruna API but serves no portal of its own, so
        the window may stay empty.
      </p>
      <div class="row">
        <button @click="tab = 'status'">Go to status</button>
      </div>
    </template>
  </section>

  <section class="panel">
    <h2>Your own node</h2>
    <p class="muted">
      Running a node on this machine is optional. Enroll it once you want offline
      authoring, a local S3 endpoint, or the portal without a network.
    </p>
    <div class="row">
      <button @click="tab = 'enroll'">Enroll this device</button>
    </div>
  </section>
</template>
