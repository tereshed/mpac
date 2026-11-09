export interface ActivityJob {
  id: string
  name: string
  kind: string
  status: 'running' | 'scheduled' | 'completed' | 'failed' | 'unknown'
  observed_at: number
  next_run_at: number | null
}
export interface ActivityState {
  adapter: string
  capabilities: { foreground: boolean; questions: boolean; approval: boolean; results: boolean; background: boolean }
  phase: 'unknown' | 'working' | 'idle' | 'stopped'
  attention: null | { kind: 'question' | 'permission' | 'input'; certainty: 'requested' | 'confirmed'; since: number }
  result: { revision: number; read_revision: number; at: number | null }
  background: { coverage: 'unsupported' | 'awaiting_snapshot' | 'snapshot' | 'unknown'; observed_at: number | null; jobs: ActivityJob[] }
  updated_at: number | null
  source: string
  tools?: { id: string; name: string; started_at: number }[]
  tools_observed_at?: number | null
}
