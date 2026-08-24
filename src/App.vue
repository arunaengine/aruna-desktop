<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import EnrollView from './views/EnrollView.vue'
import SettingsView from './views/SettingsView.vue'
import StatusView from './views/StatusView.vue'
import PortalView from './views/PortalView.vue'
import WelcomeView from './views/WelcomeView.vue'
import { boot, notice, report, tab } from './lib/store'
import type { Tab } from './lib/store'

const version = ref('')
const tabs: Array<{ id: Tab; label: string }> = [
  { id: 'status', label: 'Status' },
  { id: 'enroll', label: 'Enroll' },
  { id: 'settings', label: 'Settings' },
]

onMounted(async () => {
  try {
    version.value = await getVersion()
    await boot()
  } catch (error) {
    report(error)
  }
})
</script>

<template>
  <header class="bar">
    <h1>Aruna Desktop</h1>
    <span class="version">{{ version }}</span>
    <nav>
      <button
        v-for="entry in tabs"
        :key="entry.id"
        :class="{ active: tab === entry.id }"
        @click="tab = entry.id"
      >
        {{ entry.label }}
      </button>
    </nav>
  </header>

  <p v-if="notice" class="notice">
    {{ notice }}
    <button class="link" @click="notice = ''">dismiss</button>
  </p>

  <main class="page">
    <WelcomeView v-if="tab === 'welcome'" />
    <PortalView v-else-if="tab === 'portal'" />
    <StatusView v-else-if="tab === 'status'" />
    <EnrollView v-else-if="tab === 'enroll'" />
    <SettingsView v-else />
  </main>
</template>
