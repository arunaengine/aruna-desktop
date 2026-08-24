import { ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import * as api from './api'
import type {
  EnrollInvite,
  EnrollProgress,
  NodeStatus,
  PortalOptions,
  PortalTarget,
} from './types'

export type Tab = 'welcome' | 'portal' | 'status' | 'enroll' | 'settings'

// Why a portal window did not open, kept for the in-app error page.
export interface PortalIssue {
  target: PortalTarget
  url: string
  reason: string
  options: PortalOptions
}

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
export const portals = ref<PortalOptions>({ remote: null, local: null, chosen: null })
export const portalIssue = ref<PortalIssue | null>(null)
export const tab = ref<Tab>('status')

export function report(error: unknown) {
  notice.value = error instanceof Error ? error.message : String(error)
}

export async function refresh() {
  status.value = await api.nodeStatus()
}

export async function refreshPortals() {
  portals.value = await api.portalOptions()
}

// The remembered realm changes on a connect and on an enrollment, and both move
// what the portal button opens.
export async function syncRealm() {
  realm.value = (await api.settingsGet()).realmUrl
  await refreshPortals()
}

// Opens a portal window, or lands on the in-app error page: a target that
// refuses the connection must never show the webview's own failure.
export async function openPortal(target?: PortalTarget) {
  try {
    await refreshPortals()
    const plan = await api.portalOpen(target)
    portalIssue.value = null
    if (tab.value === 'portal') {
      tab.value = 'status'
    }
    return plan
  } catch (error) {
    const wanted = target ?? portals.value.chosen ?? 'remote'
    portalIssue.value = {
      target: wanted,
      url: (wanted === 'local' ? portals.value.local : portals.value.remote) ?? 'not configured',
      reason: error instanceof Error ? error.message : String(error),
      options: portals.value,
    }
    tab.value = 'portal'
    return null
  }
}

// Folds one observation round into the view; the backend owns the transitions.
export async function advance() {
  progress.value = await api.enrollPoll()
}

// Subscribes to the backend events and picks up a deep link that arrived
// before this window was ready. The backend decides which view that leaves the
// app on.
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
  await syncRealm()
  invite.value = await api.enrollPending()
  tab.value = await api.launchRoute()
}
