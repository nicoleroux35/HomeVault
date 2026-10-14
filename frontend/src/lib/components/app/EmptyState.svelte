<!-- ========================================================================
     File          : EmptyState.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The one standard empty/no-data state: tinted icon tile over a title,
       optional description and optional action cluster.

     Dependencies:
       - svelte (>= 5), theme/Icon

     Usage:
       `import { EmptyState } from '$lib/components/app'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — EmptyState (page composite, `app/` layer)
   *
   * The one standard empty/loading-absent state: a 56px tinted icon tile over
   * a title + optional description + optional action cluster.
   */
  import type { Snippet } from 'svelte';
  import Icon from '$lib/components/theme/Icon.svelte';

  interface Props {
    /** Sprite icon name for the tile glyph. */
    icon?: string;
    /** Custom glyph, wins over `icon`. */
    iconSnippet?: Snippet;
    /** Main message. */
    title: string;
    /** Optional supporting line. */
    description?: string;
    /** Right/below actions (buttons, links). */
    action?: Snippet;
    /** Compact variant for inline/table empty cells. */
    compact?: boolean;
    /** Test hook rendered on the root. */
    'data-testid'?: string;
  }

  let { icon, iconSnippet, title, description, action, compact = false, 'data-testid': testId }: Props = $props();
</script>

<div class="hv-empty-state flex flex-col items-center justify-center text-center px-6 py-10" data-testid={testId}>
  {#if iconSnippet || icon}
    <span class="hv-empty-icon grid place-items-center rounded-xl bg-[var(--hv-shell-fill)] text-[var(--hv-text-tertiary)] {compact ? 'w-10 h-10' : 'w-14 h-14'}">
      {#if iconSnippet}
        {@render iconSnippet()}
      {:else}
        <Icon name={icon!} size={compact ? 20 : 26} stroke={1.8} />
      {/if}
    </span>
  {/if}
  <h3 class="mt-3 text-base font-semibold text-foreground">{title}</h3>
  {#if description}
    <p class="mt-1 max-w-sm text-c-base text-muted-foreground">{description}</p>
  {/if}
  {#if action}
    <div class="mt-4 flex items-center gap-2">
      {@render action()}
    </div>
  {/if}
</div>