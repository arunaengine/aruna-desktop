import { invoke } from '@tauri-apps/api/core'
import type {
  EnrollInvite,
  EnrollProgress,
  HealthReport,
  LogPage,
  NodeSettings,
  NodeStatus,
  Paths,
} from './types'

export const nodeStatus = () => invoke<NodeStatus>('node_status')
export const nodeStart = () => invoke<NodeStatus>('node_start')
export const nodeStop = () => invoke<NodeStatus>('node_stop')
export const nodeRestart = () => invoke<NodeStatus>('node_restart')
export const nodeLogs = (cursor: number) => invoke<LogPage>('node_logs', { cursor })
export const nodeHealth = () => invoke<HealthReport>('node_health')

export const settingsGet = () => invoke<NodeSettings>('settings_get')
export const settingsSave = (settings: NodeSettings) =>
  invoke<NodeSettings>('settings_save', { settings })
export const pathsGet = () => invoke<Paths>('paths_get')

export const enrollPending = () => invoke<EnrollInvite | null>('enroll_pending')
export const enrollAccept = () => invoke<EnrollProgress>('enroll_accept')
export const enrollPaste = (text: string, seed: string, realm: string) =>
  invoke<EnrollProgress>('enroll_paste', { text, seed, realm })
export const enrollPoll = () => invoke<EnrollProgress>('enroll_poll')

export const portalOpen = () => invoke<void>('portal_open')
