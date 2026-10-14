<!-- ========================================================================
     File          : ConfirmDialog.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       App-layer confirm/alert dialog replacing the native window.confirm /
       window.alert everywhere: wraps theme/Modal so focus is trapped and
       restored, Escape closes, and the trigger regains focus (WCAG baseline
       native dialogs break). Driven by createConfirmState().

     Dependencies:
       - svelte (>= 5), theme/Modal, utils/confirmDialog.svelte.ts

     Usage:
       `<ConfirmDialog {dlg} onconfirm oncancel />` with dlg from
       createConfirmState().
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — Confirm/Alert dialog (`app/` layer)
   *
   * Replaces the native `window.confirm()` / `window.alert()` calls across
   * the apps. It wraps `theme/Modal` (bits-ui Dialog): focus is trapped and
   * restored on close, Escape closes, and the trigger's focus comes back —
   * the WCAG 2.2 / EN 301 549 baseline native dialogs break.
   *
   * Two modes (from `dlg.mode`):
   *   - `confirm` — Cancel + (danger|primary) action buttons.
   *   - `alert`   — a single OK button (dismisses via `onconfirm`).
   *
   * The whole `dlg` state object is passed in (not open/message/… props
   * individually) because the parent screens are legacy-mode components: a
   * plain `open={dlg.open}` read there does not invalidate (Svelte only
   * tracks rune-state reads it can statically attribute, and the factory
   * call is opaque to the legacy compiler). Inside this runes component the
   * reads go through `$derived`, the pattern already proven by
   * `SettingsSaveButton.svelte` (`$derived(getSaveStatus())`).
   */
  import { Modal, Button } from '$lib/components/theme';
  import { translate } from '$lib/stores/i18n';
  import type { ConfirmDialogState } from '$lib/utils/confirmDialog.svelte';

  let {
    dlg,
    title = translate('common.confirm'),
    confirmLabel = '',
    cancelLabel = '',
    onconfirm,
    oncancel,
  }: {
    dlg: ConfirmDialogState;
    title?: string;
    /** Defaults to `common.confirm` (or `common.close` in alert mode). */
    confirmLabel?: string;
    /** Defaults to `common.cancel`. */
    cancelLabel?: string;
    onconfirm?: () => void;
    oncancel?: () => void;
  } = $props();

  let show = $derived(dlg.open);
  let message = $derived(dlg.message);
  let currentMode = $derived(dlg.mode);
  let isDanger = $derived(dlg.danger);
</script>

<Modal
  open={show}
  {title}
  size="sm"
  onclose={() => (currentMode === 'alert' ? onconfirm?.() : oncancel?.())}
>
  <p class="m-0 text-sm leading-relaxed text-[var(--hv-shell-text)]" data-testid="confirm-dialog-message">{message}</p>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      {#if currentMode === 'confirm'}
        <Button variant="ghost" onclick={() => oncancel?.()}>{cancelLabel || translate('common.cancel')}</Button>
      {/if}
      <Button
        variant={isDanger ? 'danger' : 'primary'}
        onclick={() => onconfirm?.()}
      >
        {confirmLabel || (currentMode === 'alert' ? translate('common.close') : translate('common.confirm'))}
      </Button>
    </div>
  {/snippet}
</Modal>