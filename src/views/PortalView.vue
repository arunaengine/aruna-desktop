<script setup lang="ts">
import { computed } from 'vue'
import { openPortal, portalIssue, tab } from '../lib/store'

const issue = computed(() => portalIssue.value)
const other = computed(() =>
  issue.value?.target === 'local' ? issue.value.options.remote : issue.value?.options.local,
)
const otherTarget = computed(() => (issue.value?.target === 'local' ? 'remote' : 'local'))
</script>

<template>
  <section v-if="issue" class="panel hero">
    <h2>The portal did not open</h2>
    <p class="failure">{{ issue.reason }}</p>
    <dl class="grid">
      <dt>Target</dt>
      <dd>{{ issue.target === 'local' ? 'this node' : 'the realm' }}</dd>
      <dt>Address</dt>
      <dd class="mono">{{ issue.url }}</dd>
    </dl>
    <div class="row">
      <button @click="openPortal(issue.target)">Try again</button>
      <button v-if="other" @click="openPortal(otherTarget)">
        Open the {{ otherTarget === 'local' ? 'local portal instead' : 'realm portal instead' }}
      </button>
      <button class="link" @click="tab = 'welcome'">Change realm</button>
      <button class="link" @click="tab = 'status'">Back to status</button>
    </div>
    <p v-if="other" class="muted">
      {{ other }} is reachable. Ticking the offline portal in the settings makes
      that choice permanent.
    </p>
  </section>
</template>
