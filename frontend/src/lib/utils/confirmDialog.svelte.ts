/* =============================================================================
 * File          : confirmDialog.svelte.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Promise-based confirm/alert state factory replacing the native
 *   window.confirm/alert calls, which break the focus-trap and
 *   focus-restore accessibility baseline. Each component instance creates
 *   its own state (two apps on the same desktop never share a dialog); the
 *   returned object is a single Svelte 5 $state proxy whose reads stay
 *   reactive — read through the proxy, never through extracted getters.
 *
 * Dependencies:
 *   - svelte (>= 5, runes)
 *
 * Usage:
 *   `const dlg = createConfirmState();` then `await dlg.ask(message)` and
 *   bind `<ConfirmDialog {dlg} ...>`.
 * =============================================================================
 */

/**
 * HOMEVAULT — promise-based confirm/alert state factory
 *
 * Replaces the native `window.confirm()` / `window.alert()` calls that break
 * the WCAG 2.2 / EN 301 549 focus-trap + focus-restore baseline (theme/Modal
 * wraps bits-ui Dialog, which traps and restores focus; native dialogs do
 * not). The state object is created per component instance (the factory is
 * called during the owning component's init), so two apps on the same desktop
 * never share one dialog.
 *
 * The returned object is a single `$state` proxy (same shape as
 * `$lib/stores/settingsDraft.svelte.ts`): consumers bind
 * `<ConfirmDialog open={dlg.open} …>` and those reads stay tracked, so
 * `ask()` / `settle()` re-render the dialog. Do NOT return bare properties
 * or getters over `$state` locals here — the API consumers must read through
 * the proxy for Svelte to pick up the change (verified 2026-09-22: getters
 * only ever delivered the initial value).
 *
 * Usage (in any app screen):
 *
 *   const dlg = createConfirmState();
 *   if (!(await dlg.ask($t('app.delete_confirm')))) return;   // instead of confirm()
 *   dlg.notify($t('app.saved'));                              // instead of alert()
 *
 *   <ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)}
 *     oncancel={() => dlg.settle(false)} />
 */
export interface ConfirmDialogState {
  open: boolean;
  message: string;
  danger: boolean;
  mode: 'confirm' | 'alert';
  /**
   * When non-empty, the dialog renders a text field and the confirm button
   * stays disabled until the user types this exact string — the typed
   * confirmation reserved for irreversible actions (destroying a virtual
   * disk, wiping a device).
   */
  requireText: string;
  /** What the user has typed into the confirmation field. */
  typed: string;
  /**
   * Ask a yes/no question; resolves `true` when the user confirms.
   *
   * `requireText` turns the dialog into a typed confirmation: pass the name
   * of the thing about to be destroyed, and the confirm button only enables
   * once it has been typed exactly.
   */
  ask(msg: string, opts?: { danger?: boolean; requireText?: string }): Promise<boolean>;
  /** Show a single-OK notice (replaces `alert()`). */
  notify(msg: string): void;
  /** Close the dialog and (for confirm mode) resolve the pending promise. */
  settle(value: boolean): void;
}

export function createConfirmState(): ConfirmDialogState {
  const state = $state({
    open: false,
    message: '',
    danger: true,
    mode: 'confirm' as 'confirm' | 'alert',
    requireText: '',
    typed: '',
  });
  let resolve: ((value: boolean) => void) | null = null;

  /** Ask a yes/no question; resolves `true` when the user confirms. */
  function ask(
    msg: string,
    opts: { danger?: boolean; requireText?: string } = {},
  ): Promise<boolean> {
    state.message = msg;
    state.danger = opts.danger ?? true;
    state.mode = 'confirm';
    state.requireText = opts.requireText ?? '';
    state.typed = '';
    state.open = true;
    return new Promise<boolean>((res) => {
      resolve = res;
    });
  }

  /** Show a single-OK notice (replaces `alert()`). */
  function notify(msg: string): void {
    state.message = msg;
    state.danger = false;
    state.mode = 'alert';
    state.requireText = '';
    state.typed = '';
    state.open = true;
  }

  /** Close the dialog and (for confirm mode) resolve the pending promise. */
  function settle(value: boolean): void {
    state.open = false;
    resolve?.(value);
    resolve = null;
  }

  return Object.assign(state, { ask, notify, settle });
}