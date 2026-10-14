/* =============================================================================
 * File          : backupJobs.svelte.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Persistent backup/restore job tracking for the floating progress
 *   island. Module-level state (not per-component) so a job keeps reporting
 *   while the operator navigates elsewhere; trackJob subscribes to the
 *   job's SSE progress stream, jobs settle to done/failed and stay listed
 *   until dismissed. Tracking is idempotent per job id.
 *
 * Dependencies:
 *   - svelte (>= 5, runes)
 *
 * Usage:
 *   trackJob(jobId, label) right after starting a backup/restore;
 *   trackedJobs()/activeTrackedJobs() feed the island.
 * =============================================================================
 */

/**
 * Persistent backup/restore job tracking for the floating progress island.
 *
 * Module-level state (not per-component): a backup keeps reporting while the
 * operator navigates to another app, and the island re-renders wherever it is
 * mounted. `trackJob` subscribes to the job's SSE progress stream; jobs
 * settle to `done`/`failed` and stay listed until dismissed.
 */
import { api } from '$lib/services/api';

export interface TrackedJob {
  jobId: string;
  label: string;
  progress: number;
  message: string;
  state: 'running' | 'done' | 'failed';
}

const jobs = $state<TrackedJob[]>([]);
const seen = new Set<string>();

export function trackedJobs(): TrackedJob[] {
  return jobs;
}

/** Subscribe to a progress job and surface it in the island. Idempotent. */
export function trackJob(jobId: string, label: string): void {
  if (!jobId || seen.has(jobId)) return;
  seen.add(jobId);
  const entry: TrackedJob = { jobId, label, progress: 0, message: label, state: 'running' };
  jobs.push(entry);
  api.subscribeToProgress(
    jobId,
    (event: any) => {
      entry.progress = typeof event.progress === 'number' ? event.progress : entry.progress;
      entry.message = event.message || entry.message;
      if (event.state === 'failed') entry.state = 'failed';
    },
    () => {
      entry.progress = 100;
      entry.state = 'done';
    },
    () => {
      entry.state = 'failed';
    },
  );
}

export function dismissJob(jobId: string): void {
  const i = jobs.findIndex((j) => j.jobId === jobId);
  if (i >= 0) jobs.splice(i, 1);
}

export function activeTrackedJobs(): TrackedJob[] {
  return jobs.filter((j) => j.state === 'running');
}
