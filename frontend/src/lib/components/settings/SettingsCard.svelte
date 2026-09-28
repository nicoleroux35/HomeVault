<script lang="ts">
  import type { Snippet } from 'svelte';
  import { cn } from '$lib/utils.js';
  import { settingsCard, settingsCardFooter } from './vocabulary';

  let {
    icon,
    iconVariant = 'blue',
    accent,
    title,
    subtitle,
    children,
    footer,
    class: klass = '',
  }: {
    icon: any;
    iconVariant?: 'blue' | 'green' | 'red' | 'orange' | 'purple';
    /** Explicit category colour. Overrides `iconVariant`; pass a
     *  `--hv-settings-cat-*` token so the card stays on the palette. */
    accent?: string;
    title: string;
    subtitle?: string;
    children?: Snippet;
    footer?: Snippet;
    /** Extra classes merged onto the card root (e.g. `mt-3` spacing). */
    class?: string;
  } = $props();

  // Svelte only treats a template tag as a dynamic component if it's a
  // capitalized identifier — `icon` (lowercase, from props) rendered as
  // `<icon>` compiles as a literal, nonexistent HTML element instead.
  let Icon = $derived(icon);

  const VARIANT_COLOR: Record<string, string> = {
    blue: 'var(--hv-primary)',
    green: 'var(--hv-success)',
    red: 'var(--hv-error)',
    orange: 'var(--hv-warning)',
    purple: 'var(--hv-settings-cat-appearance)',
  };

  const tint = $derived(accent ?? VARIANT_COLOR[iconVariant] ?? VARIANT_COLOR.blue);
</script>

<!--
  SettingsCard — the sub-card shell used by the six embedded settings cards
  (HTTPS/TLS, Antivirus, Anti-Ransomware, Firewall, UPS, SNMP).

  It renders the same section + row vocabulary as the rest of the Settings
  screen: the shared `--hv-settings-*` card, a 36px icon tile in the card's
  own colour at 9%, and a 16px/700 title. `data-testid="settings-card"` /
  `settings-card-title` are the E2E contract — the security and monitoring
  categories have no hand-rolled section label, so the card's own heading is
  what proves they rendered (tests 2.1, 2.5, 12.1).

  Inner content is built by the caller out of `vocabulary.ts`'s row/button
  strings, so a card here and a card in `Settings.svelte` cannot drift.
-->
<div class={cn(settingsCard, 'settings-card', klass)} data-testid="settings-card">
  <div class="settings-card-header flex items-center gap-3 py-4 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
    <span
      class="settings-card-icon grid place-items-center w-9 h-9 rounded-xl flex-none"
      style="background: color-mix(in srgb, {tint} var(--hv-settings-cat-tint), transparent); color: {tint}"
    >
      <Icon size={20} />
    </span>
    <div class="min-w-0">
      <!-- The `!` flags are load-bearing — `styles/theme/base.css` sets an
           unlayered `h2 { font-size: … }`, which outranks every layered
           Tailwind utility. See the same note in `routes/Settings.svelte`.
           `text-md` is the design's 16px/700 card title (`text-base` is
           14px). -->
      <h2 data-testid="settings-card-title" class="settings-card-title m-0 text-md! font-bold! leading-6! text-[var(--hv-settings-title)]!">{title}</h2>
      {#if subtitle}
        <p class="settings-card-subtitle m-0 mt-0.5 text-xs leading-4 text-[var(--hv-settings-label)]">{subtitle}</p>
      {/if}
    </div>
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
