<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'

const version = ref('')

onMounted(async () => {
  version.value = await getVersion()
})
</script>

<template>
  <header class="bar">
    <h1>Aruna Desktop</h1>
    <span class="version">{{ version }}</span>
  </header>
  <main class="page">
    <p>The native control window for a local Aruna user node.</p>
  </main>
</template>
