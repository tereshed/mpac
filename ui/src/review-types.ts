import type { Session } from './api'

export type ReviewSide = 'old' | 'new' | 'both'
export interface ReviewLine { index: number; old_line: number | null; new_line: number | null; kind: 'context' | 'addition' | 'deletion'; text: string; hunk: number }
export interface ReviewBlock { id: number; path: string; section: 'staged' | 'working' | 'untracked'; lines: ReviewLine[]; lines_truncated?: boolean; raw?: string }
export interface StructuredReview { snapshot: string; head: string; truncated: boolean; blocks: ReviewBlock[] }
export interface SourceLines { snapshot: string; path: string; lines: ReviewLine[]; lines_truncated?: boolean }
export type ReviewAnchor = { kind: 'file' } | { kind: 'range'; block_id?: number; start_row: number; end_row: number; side: ReviewSide }
export interface ReviewComment {
  id: string; project_id: string; root_id: string; path: string; source: 'diff' | 'file'; snapshot: string;
  text: string; revision: number; anchor: ReviewAnchor; excerpt: string;
  old_start: number | null; old_end: number | null; new_start: number | null; new_end: number | null;
  deleted: boolean; created_at: number | string; updated_at: number | string; stale?: boolean;
  excerpt_truncated?: boolean;
  deliveries?: { session_id: string; revision: number; batch_id: string; at: number | string; status: 'handed_off' }[];
}
export interface ReviewBatch {
  id: string; project_id: string; session_id: string; comments: { id: string; revision: number }[];
  message: string; text: string; status: 'draft' | 'preparing' | 'prepared' | 'handed_off' | 'uncertain'; created_at: number | string;
  attachment_path?: string | null;
}
export interface ReviewCatalog { comments: ReviewComment[]; batches: ReviewBatch[] }
export interface LaunchForReviewRequest { rootId: string; requiredRootIds: string[]; onReady: (session: Session) => void }
export type LaunchForReview = (request: LaunchForReviewRequest) => void
