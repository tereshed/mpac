export interface CommitPreview {
  head: string; branch: string | null; snapshot: string; paths: string[];
  files: { path: string; status: string; diff: string }[]; diff: string; blocked_reason: string | null;
}
export interface CommitResult {
  committed: boolean; head: string; previous_head: string; paths: string[];
  warning: string | null; recovery_index: string | null;
}
