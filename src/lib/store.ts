import { ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import * as api from './api'
import type { EnrollPayload, NodeStatus } from './types'

export type Tab = 'status' | 'enroll' | 'settings'

const idle: NodeStatus = {
  state: 'stopped',
  pid: null,
  binary: null,
  detail: null,
  restarts: 0,
  lastExit: null,
}

export const status = ref<NodeStatus>(idle)
export const notice = ref('')
export const pending = ref<EnrollPayload | null>(null)
export const tab = ref<Tab>('status')

export function report(error: unknown) {
  notice.value = error instanceof Error ? error.message : String(error)
}

export async function refresh() {
  status.value = await api.nodeStatus()
}

// Subscribes to the backend events and picks up a deep link that arrived
// before this window was ready.
export async function boot() {
  await listen<NodeStatus>('node-status', (event) => {
    status.value = event.payload
  })
  await listen<EnrollPayload>('enroll-payload', (event) => {
    pending.value = event.payload
    tab.value = 'enroll'
  })
  await refresh()
  const stashed = await api.enrollTake()
  if (stashed) {
    pending.value = stashed
    tab.value = 'enroll'
  }
}
