<!-- ========================================================================
     File          : StatCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The one standard metric tile (previously re-implemented by
       Dashboard/Storage/Health): tinted icon tile beside a label and big
       value, optional footer note, five semantic tones. With onClick it
       becomes an interactive button with a 44px-min, keyboard-activatable
       target.

     Dependencies:
       - svelte (>= 5), theme/Icon

     Usage:
       `import { StatCard } from '$lib/components/app'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — StatCard (page composite, `app/` layer)
   *
   * The one standard metric tile (the pattern Dashboard / Storage / Health
   * used to re-implement per app): a 44px tinted icon tile beside a label +
   * big value, plus an optional footer note. Five tones map onto the
   * appliance semantic tints. Pass `onClick` to make the tile an interactive
   * button (44px-min target, keyboard-activatable).
   */
  import type { Snippet } from 'svelte';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { cn } from '$lib/utils.js';

  type Tone = 'default' | 'success' | 'warning' | 'error' | 'info';

  const toneTile: Record<Tone, string> = {
    default: 'bg-[var(--hv-shell-fill)] text-[var(--hv-text-primary)]',
    success: 'bg-[var(--hv-success-light)] text-[var(--hv-success-text)]',
    warning: 'bg-[var(--hv-warning-light)] text-[var(--hv-warning-text)]',
    error: 'bg-[var(--hv-error-light)] text-[var(--hv-error-text)]',
    info: 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]',
  };

  interface Props {
    /** Short metric label. */
    label: string;
    /** The metric value (string or pre-formatted snippet). */
    value?: string | Snippet;
    /** Optional sprite icon name for the tile glyph. */
    icon?: string;
    /** Custom glyph, wins over `icon`. */
    iconSnippet?: Snippet;
    /** Tint family of the icon tile. */
    tone?: Tone;
    /** Optional footer note (status text, unit, trend). */
    sub?: string | Snippet;
    /** Make the tile interactive. */
    onClick?: () => void;
    /** Extra class(es) merged onto the tile. */
    class?: string;
    /** Test hook rendered on the tile. */
    'data-testid'?: string;
  }

  let {
    label,
    value,
    icon,
    iconSnippet,
    tone = 'default',
    sub,
    onClick,
    class: extraClass = '',
    'data-testid': testId,
  }: Props = $props();

  const tileClass = $derived(toneTile[tone]);

  function baseClass(): string {
    return cn(
      'hv-stat-card flex items-center gap-4 rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] px-5 py-4 text-left',
      onClick ? 'cursor-pointer transition-colors hover:border-[var(--hv-shell-border-strong)] focus-visible:outline-2 focus-visible:outline-[var(--hv-primary)] focus-visible:outline-offset-2' : '',
      extraClass,
    );
  }
</script>

{#if onClick}
  <button class={baseClass()} type="button" data-testid={testId} onclick={onClick}>
    {#if iconSnippet || icon}
      <span class={cn('hv-stat-icon grid place-items-center w-11 h-11 rounded-lg flex-none', tileClass)}>
        {#if iconSnippet}
          {@render iconSnippet()}
        {:else}
          <Icon name={icon!} size={22} stroke={1.8} />
        {/if}
      </span>
    {/if}
    <span class="min-w-0">
      <span class="block text-c-sm font-medium text-muted-foreground truncate">{label}</span>
      {#if value}
        <span class="block text-xl font-semibold leading-tight text-foreground truncate">
          {#if typeof value === 'string'}{value}{:else}{@render value()}{/if}
        </span>
      {/if}
      {#if sub}
        <span class="block text-c-xs text-muted-foreground truncate">
          {#if typeof sub === 'string'}{sub}{:else}{@render sub()}{/if}
        </span>
      {/if}
    </span>
  </button>
{:else}
  <div class={baseClass()} data-testid={testId}>
    {#if iconSnippet || icon}
      <span class={cn('hv-stat-icon grid place-items-center w-11 h-11 rounded-lg flex-none', tileClass)}>
        {#if iconSnippet}
          {@render iconSnippet()}
        {:else}
          <Icon name={icon!} size={22} stroke={1.8} />
        {/if}
      </span>
    {/if}
    <div class="min-w-0">
      <div class="text-c-sm font-medium text-muted-foreground truncate">{label}</div>
      {#if value}
        <div class="text-xl font-semibold leading-tight text-foreground truncate">
          {#if typeof value === 'string'}{value}{:else}{@render value()}{/if}
        </div>
      {/if}
      {#if sub}
        <div class="text-c-xs text-muted-foreground truncate">
          {#if typeof sub === 'string'}{sub}{:else}{@render sub()}{/if}
        </div>
      {/if}
    </div>
  </div>
{/if}