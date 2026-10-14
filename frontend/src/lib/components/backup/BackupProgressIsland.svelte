<!-- ========================================================================
     File          : BackupProgressIsland.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Floating persistent island for backup/restore progress: lives outside
       the Backup window so a long run stays visible while the operator
       works elsewhere; driven by the module-level backupJobs store.

     Dependencies:
       - svelte (>= 5), stores/backupJobs.svelte, stores/i18n

     Usage:
       Mounted by the desktop shell.
     ======================================================================== -->
<script lang="ts">
  import { trackedJobs, dismissJob } from '$lib/stores/backupJobs.svelte';
  import { t } from '$lib/stores/i18n';
  import { IconCheck, IconLoader, IconX, IconAlertCircle } from '@tabler/icons-svelte';

  /** Floating persistent island for backup/restore progress. Lives outside the
   *  Backup window so a long run stays visible while doing something else. */
  let jobs = $derived(trackedJobs());
</script>

{#if jobs.length > 0}
  <aside data-testid="backup-progress-island" class="fixed bottom-[calc(var(--hv-chrome-dock-h)+12px)] right-4 z-[var(--hv-z-notification)] w-72 max-w-[calc(100vw-2rem)] rounded-xl border border-[var(--hv-chrome-surface-border)] bg-[var(--hv-shell-glass)] shadow-[var(--hv-chrome-surface-shadow)]" role="status" aria-live="polite">
    <div class="flex flex-col gap-2 p-3">
      {#each jobs as job (job.jobId)}
        <div class="flex items-start gap-2" data-testid="backup-progress-item" data-state={job.state}>
          {#if job.state === 'running'}
            <IconLoader size={16} class="mt-0.5 shrink-0 animate-spin text-[var(--hv-primary)]" />
          {:else if job.state === 'done'}
            <IconCheck size={16} class="mt-0.5 shrink-0 text-[var(--hv-success)]" />
          {:else}
            <IconAlertCircle size={16} class="mt-0.5 shrink-0 text-[var(--hv-error)]" />
          {/if}
          <div class="min-w-0 flex-1">
            <p class="m-0 truncate text-c-sm font-medium text-[var(--hv-shell-text-strong)]">{job.label}</p>
            <p class="m-0 truncate text-c-xs text-[var(--hv-shell-text-2)]">{job.message}</p>
            {#if job.state === 'running'}
              <div class="mt-1 h-1.5 overflow-hidden rounded-full bg-[var(--hv-shell-border-soft)]" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(job.progress)}>
                <div class="h-full rounded-full bg-[var(--hv-primary)] transition-all" style="width: {job.progress}%"></div>
              </div>
            {/if}
          </div>
          {#if job.state !== 'running'}
            <button class="grid h-8 w-8 flex-none place-items-center rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hv-ic" onclick={() => dismissJob(job.jobId)} aria-label={$t('common.close')}><IconX size={14} /></button>
          {/if}
        </div>
      {/each}
    </div>
  </aside>
{/if}
