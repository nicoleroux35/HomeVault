<!-- ========================================================================
     File          : WhatsNewModal.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       "What's New" modal, shown once per release per user: Desktop.svelte
       mounts it with the payload from GET /api/whats-new (title +
       highlights) and posts the acknowledgement when dismissed.

     Dependencies:
       - svelte (>= 5), theme/Modal

     Usage:
       Mounted by Desktop.svelte after login.
     ======================================================================== -->
<script lang="ts">
  /**
   * "What's New" modal — shown once per release, per user.
   *
   * `Desktop.svelte` mounts this and passes `open`/`notes`. The parent fetches
   * `GET /api/whats-new` after login; when `seen` is false the modal opens with
   * the release highlights. Closing it (button, Escape, backdrop) calls
   * `onclose`, and the parent posts `POST /api/whats-new/seen` so it stays
   * dismissed across browsers and devices.
   *
   * Uses the shared `theme/Modal` (bits-ui Dialog) so focus trapping, Escape
   * and focus restoration to the trigger are handled for free.
   */
  import Modal from '$lib/components/theme/Modal.svelte';
  import Button from '$lib/components/theme/Button.svelte';
  import { t } from '$lib/stores/i18n';

  let {
    open = false,
    version = '',
    title = '',
    highlights = [],
    onclose,
  }: {
    open?: boolean;
    version?: string;
    title?: string;
    highlights?: string[];
    onclose?: () => void;
  } = $props();
</script>

<Modal
  {open}
  size="lg"
  title={$t('whats_new.title')}
  onclose={() => onclose?.()}
>
  <div class="hv-whats-new" data-testid="whats-new-modal">
    {#if title}
      <p class="hv-whats-new-subtitle">{title}</p>
    {/if}
    {#if version}
      <span class="hv-whats-new-version">{$t('whats_new.version', { version })}</span>
    {/if}
    <ul class="hv-whats-new-list">
      {#each highlights as highlight (highlight)}
        <li>{highlight}</li>
      {/each}
    </ul>
  </div>

  {#snippet footer()}
    <Button variant="primary" onclick={() => onclose?.()} data-testid="whats-new-dismiss">
      {$t('whats_new.got_it')}
    </Button>
  {/snippet}
</Modal>

<style>
  .hv-whats-new {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .hv-whats-new-subtitle {
    margin: 0;
    font-size: var(--hv-text-base, 1rem);
    color: var(--hv-shell-text-2);
  }

  .hv-whats-new-version {
    align-self: flex-start;
    padding: 0.125rem 0.5rem;
    border-radius: 999px;
    background: var(--hv-primary-selected-bg);
    color: var(--hv-primary);
    font-size: var(--hv-text-sm, 0.8125rem);
    font-weight: 600;
  }

  .hv-whats-new-list {
    margin: 0;
    padding-left: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    color: var(--hv-shell-text);
    line-height: 1.5;
  }
</style>
