export type NodeState =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'restarting'
  | 'failed'
  | 'binaryMissing'

export interface NodeStatus {
  state: NodeState
  pid: number | null
  binary: string | null
  detail: string | null
  restarts: number
  lastExit: number | null
}

export type LogStream = 'stdout' | 'stderr' | 'desktop'

export interface LogLine {
  seq: number
  stream: LogStream
  text: string
}

export interface LogPage {
  lines: LogLine[]
  cursor: number
  truncated: boolean
}

export interface Probe {
  ok: boolean
  status: number | null
  body: unknown
  error: string | null
}

export interface HealthReport {
  health: Probe
  ready: Probe
}

export interface NodePorts {
  portal: number
  api: number
  ops: number
  s3: number
  p2p: number
}

export interface NodeSettings {
  binaryPath: string | null
  storagePath: string | null
  portalDir: string | null
  ports: NodePorts
  s3Enabled: boolean
  shutdownGraceSecs: number
  // Always null over the bridge: the backend redacts the one-time secret.
  onboardingSecret: string | null
  seedUrl: string | null
  realm: string | null
  realmUrl: string | null
  enrolled: boolean
  preferLocalPortal: boolean
  autoStart: boolean
  extraEnv: Record<string, string>
}

export interface Paths {
  configFile: string
  dataDir: string
  storageDir: string
  envFile: string
}

export type PortalTarget = 'remote' | 'local'

export interface PortalPlan {
  target: PortalTarget
  url: string
}

export interface PortalOptions {
  remote: string | null
  local: string | null
  chosen: PortalTarget | null
}

export interface RealmTarget {
  url: string
  realm: string | null
  apiVersion: string | null
  portal: boolean
}

export interface EnrollInvite {
  seed: string
  realm: string | null
}

export type EnrollPhase =
  | 'idle'
  | 'received'
  | 'starting'
  | 'waiting'
  | 'visible'
  | 'done'
  | 'failed'

export interface EnrollProgress {
  phase: EnrollPhase
  realm: string | null
  seed: string | null
  detail: string | null
}
