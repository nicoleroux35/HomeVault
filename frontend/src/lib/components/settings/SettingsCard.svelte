<!-- ========================================================================
     File          : SettingsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The standard settings card shell: a two-line header (title over its
       description) above a body, with the shared class vocabulary so every
       card — rail groups and embedded sub-cards alike — looks identical.

     Dependencies:
       - svelte (>= 5), ./vocabulary

     Usage:
       Base of every Settings group/sub-card.
     ======================================================================== -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { cn } from '$lib/utils.js';
  import { settingsCard, settingsCardFooter } from './vocabulary';

  let {
    title,
    subtitle,
    children,
    footer,
    class: klass = '',
  }: {
    title: string;
    subtitle?: string;
    children?: Snippet;
    footer?: Snippet;
    /** Extra classes merged onto the card root (e.g. `mt-3` spacing). */
    class?: string;
  } = $props();
</script>

<!--
  SettingsCard — the sub-card shell used by the seven embedded settings cards
  (HTTPS/TLS, Antivirus, Anti-Ransomware, Firewall, AI Assistant, UPS, SNMP).

  It renders the same section + row vocabulary as the rest of the Settings
  screen: the shared `--hv-settings-*` card and a two-line header — title over
  its description, no icon tile (removed 2026-10-19: the category identity
  already lives in the rail, and a 36px tile per card made the panel scroll
  far more than its content needed). `data-testid="settings-card"` /
  `settings-card-title` are the E2E contract — the security and monitoring
  categories have no hand-rolled section label, so the card's own heading is
  what proves they rendered (tests 2.1, 2.5, 12.1).

  Inner content is built by the caller out of `vocabulary.ts`'s row/button
  strings, so a card here and a card in `Settings.svelte` cannot drift.
-->
<div class={cn(settingsCard, 'settings-card', klass)} data-testid="settings-card">
  <div class="settings-card-header py-2.5 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
    <!-- The `!` flags are load-bearing — `styles/theme/base.css` sets an
         unlayered `h2 { font-size: … }`, which outranks every layered
         Tailwind utility. See the same note in `routes/Settings.svelte`.
         `text-md` is the native title-3 step (15px/700) — the card title;
         `text-base` is the body step (13px). -->
    <h2 data-testid="settings-card-title" class="settings-card-title m-0 text-md! font-bold! leading-5! text-[var(--hv-settings-title)]!">{title}</h2>
    {#if subtitle}
      <!-- `mb-0!` matters as much as the `!` flags above: `base.css` sets an
           unlayered `p { margin-bottom: var(--hv-space-4) }`, which a plain
           `m-0` cannot override — the card header then carries 16px of dead
           space under its subtitle. -->
      <p class="settings-card-subtitle m-0! text-xs leading-4 text-[var(--hv-settings-label)]">{subtitle}</p>
    {/if}
  </div>

  <div class="settings-card-body">
    {#if children}
      {@render children()}
    {/if}
  </div>

  {#if footer}
    <div class={cn(settingsCardFooter, 'settings-card-footer justify-end')}>
      {@render footer()}
    </div>
  {/if}
</div>
