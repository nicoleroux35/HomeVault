<!-- ========================================================================
     File          : SettingsSaveButton.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The settings sidebar's Save control: reflects the dirty-group count,
       save-in-progress and per-group outcomes from the shared settings
       draft (Save All — no auto-save anywhere).

     Dependencies:
       - svelte (>= 5), stores/settingsDraft.svelte, ./vocabulary

     Usage:
       Fixed in the Settings sidebar footer.
     ======================================================================== -->
<script lang="ts">
  import { t } from '$lib/stores/i18n';
  import { getSaveStatus, getDirtyCount, saveAll } from '$lib/stores/settingsDraft.svelte';
  import { IconAlertTriangle, IconCheck } from '@tabler/icons-svelte';
  import { onDestroy } from 'svelte';
  import { cn } from '$lib/utils.js';
  import { settingsBtn, settingsBtnPrimary, settingsBtnDanger, settingsBtnSuccess } from './vocabulary';

  /* The one Save button for the whole Settings screen, in the rail footer —
     and the screen's only save indicator (2026-09-23): the visible status pill
     that sat above it and the "N unsaved change(s)" caption under it were
     removed, so one element carries one message.

     Its colour **and** label are the state machine (`data-state` is the E2E
     hook):
       clean   blue    "Save"          nothing staged
       dirty   red     "Save changes"  a parameter changed, flush pending
       saving  blue    "Saving…"       flush in flight (button disabled)
       saved   green   "Saved"         FLASH_MS of confirmation, then clean
       error   red     "Not saved"     a group's PUT/POST failed
     The label and the icon always change with the colour, so the state is
     never colour-alone (WCAG 1.4.1). White on the three fills measures
     6.15:1 (blue), 5.73:1 (red) and 5.14:1 (green): the blue is the live
     `--hv-primary` accent, whose shipped default has been #1E62AE since
     2026-09-23 — the legacy #4a90d9 (3.34:1, fails §1.4.3) is rewritten
     server-side by `handlers::settings::normalize_accent`.

     The sr-only `role="status"` line mirrors the label for screen readers.
     The label is stable within a state (no per-keystroke churn), so it
     announces a *state change*, not every edit.

     Runes component so `$derived(...)` tracks the store's signals — a legacy
     template read of a getter is untracked. The label text stays in the
     template (`saveLabel` snippet) so `$t(...)` goes through the same tracked
     path every other component uses — it must follow a language switch. */
  const status = $derived(getSaveStatus());
  const dirtyCount = $derived(getDirtyCount());

  /** How long the green confirmation stays up — long enough to read it. */
  const FLASH_MS = 2500;

  /** Set by a successful save; cleared by FLASH_MS or by the next edit. */
  let flashed = $state(false);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  const phase = $derived.by((): 'clean' | 'dirty' | 'saving' | 'saved' | 'error' => {
    if (status === 'saving') return 'saving';
    if (status === 'error') return 'error';
    if (dirtyCount > 0) return 'dirty';
    return flashed ? 'saved' : 'clean';
  });

  /** Exactly one skin per state — two `bg-` utilities would fight. */
  const skin = $derived(
    phase === 'dirty' || phase === 'error'
      ? settingsBtnDanger
      : phase === 'saved'
        ? settingsBtnSuccess
        : settingsBtnPrimary,
  );

  /* An edit staged during (or after) the green flash wins: a stale "Saved"
     must never sit over an unsaved change. It only writes when dirtyCount
     changes and never touches the flash timer, so it cannot race onSave. */
  $effect(() => {
    if (dirtyCount > 0) flashed = false;
  });

  onDestroy(() => clearTimeout(flashTimer));

  async function onSave() {
    const failed = await saveAll();
    if (failed > 0) return; // phase stays 'error' — red, "Not saved"
    flashed = true;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flashed = false), FLASH_MS);
  }
</script>

<!-- The label, rendered twice (button + sr-only status line): text only, so
     the live region never carries an icon or the spinner. -->
{#snippet saveLabel()}
  {#if phase === 'saving'}{$t('settings.autosave_saving')}
  {:else if phase === 'error'}{$t('settings.save_error')}
  {:else if phase === 'dirty'}{$t('settings.save_changes')}
  {:else if phase === 'saved'}{$t('settings.autosave_saved')}
  {:else}{$t('settings.save')}
  {/if}
{/snippet}

<button
  class={cn(settingsBtn, skin)}
  onclick={onSave}
  disabled={status === 'saving'}
  data-testid="settings-save"
  data-state={phase}
>
  {#if phase === 'saving'}<span class="set-save-spin"></span>
  {:else if phase === 'dirty' || phase === 'error'}<IconAlertTriangle size={16} stroke={2.5} />
  {:else}<IconCheck size={16} stroke={2.5} />
  {/if}{@render saveLabel()}
</button>

<!-- Invisible by design: the rail shows exactly one element (the button),
     while screen readers still hear every state change. -->
<span data-testid="settings-save-status" role="status" aria-live="polite" class="sr-only">{@render saveLabel()}</span>

<style>
  /* The in-flight spinner, tinted from the button's own text colour so it
     keeps its contrast on blue, red or green instead of a fixed border. */
  .set-save-spin {
    display: inline-block;
    flex: none;
    width: 12px;
    height: 12px;
    border-radius: var(--hv-radius-full);
    border: 2px solid color-mix(in srgb, currentColor 35%, transparent);
    border-top-color: currentColor;
    animation: set-spin 700ms linear infinite;
  }

  @keyframes set-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .set-save-spin {
      animation-duration: 2s;
    }
  }
</style>
