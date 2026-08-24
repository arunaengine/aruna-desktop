import { ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import * as api from './api'
import type { EnrollInvite, EnrollProgress, NodeStatus } from './types'

export type Tab = 'welcome' | 'status' | 'enroll' | 'settings'

const idle: NodeStatus = {
  state: 'stopped',
  pid: null,
  binary: null,
  detail: null,
  restarts: 0,
  lastExit: null,
}

const unenrolled: EnrollProgress = {
  phase: 'idle',
  realm: null,
  seed: null,
  detail: null,
}

export const status = ref<NodeStatus>(idle)
export const notice = ref('')
export const invite = ref<EnrollInvite | null>(null)
export const progress = ref<EnrollProgress>(unenrolled)
export const realm = ref<string | null>(null)
export const tab = ref<Tab>('status')

export function report(error: unknown) {
  notice.value = error instanceof Error ? error.message : String(error)
}

export async function refresh() {
  status.value = await api.nodeStatus()
}

// Folds one observation round into the view; the backend owns the transitions.
export async function advance() {
  progress.value = await api.enrollPoll()
}

// Subscribes to the backend events and picks up a deep link that arrived
// before this window was ready. Without a remembered realm the app lands on the
// welcome view; an arriving invitation still wins.
export async function boot() {
  await listen<NodeStatus>('node-status', (event) => {
    status.value = event.payload
  })
  await listen<EnrollInvite>('enroll-invite', (event) => {
    invite.value = event.payload
    tab.value = 'enroll'
  })
  await refresh()
  await advance()
  realm.value = (await api.settingsGet()).realmUrl
  invite.value = await api.enrollPending()
  if (invite.value) {
    tab.value = 'enroll'
  } else if (!realm.value) {
    tab.value = 'welcome'
  }
}
