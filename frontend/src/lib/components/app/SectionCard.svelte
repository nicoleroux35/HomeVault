<!-- ========================================================================
     File          : SectionCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The one standard titled card section: optional header (title,
       description, right-aligned actions) over a body, reading the appliance
       tokens directly so every section in every app is identical.

     Dependencies:
       - svelte (>= 5)

     Usage:
       `import { SectionCard } from '$lib/components/app'`.
     ======================================================================== -->
<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — SectionCard (page composite, `app/` layer)
   *
   * The one standard titled card section for every app screen: an optional
   * card header (title + description + right-aligned actions) over the body.
   * Surfaces read the appliance tokens directly (card = --hv-shell-card,
   * hairline = --hv-shell-border, corner = 12px) so every section in every
   * app is identical.
   */
  import type { Snippet } from 'svelte';
  import { cn } from '$lib/utils.js';

  interface Props {
    /** Section title (rendered in the header). */
    title?: string;
    /** Custom header title (icon + text, chips) — wins over `title`. */
    titleSnippet?: Snippet;
    /** One-line description under the title. */
    description?: string;
    /** Right-aligned header cluster (buttons, badges, counts). */
    actions?: Snippet;
    /**
     * Rule off the header from the body (hairline + compact body rhythm).
     * Default false — the header and body form one block. Owner: the Users
     * detail pane (`frontend/src/routes/Users.svelte`).
     */
    divider?: boolean;
    /** Extra class(es) merged onto the section element. */
    class?: string;
    /** Test hook rendered on the section. */
    'data-testid'?: string;
    /** The card body. */
    children: Snippet;
  }

  let { title, titleSnippet, description, actions, divider = false, class: extraClass = '', 'data-testid': testId, children }: Props = $props();

  const hasHeader = $derived(!!(title || titleSnippet || description || actions));
</script>

<section class={cn('hv-section-card rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)]', extraClass)} data-testid={testId}>
  {#if hasHeader}
    <header class={cn('hv-section-card-header flex items-start justify-between gap-4 px-5 pt-5 pb-3', divider && 'border-b border-[var(--hv-shell-border-soft)]')}>
      <div class="min-w-0">
        {#if titleSnippet}
          {@render titleSnippet()}
        {:else if title}
          <h2 class="text-lg font-semibold leading-snug text-foreground truncate">
            {title}
          </h2>
        {/if}
        {#if description}
          <p class="mt-0.5 text-sm text-muted-foreground">{description}</p>
        {/if}
      </div>
      {#if actions}
        <div class="hv-section-card-actions flex items-center gap-2 flex-none">
          {@render actions()}
        </div>
      {/if}
    </header>
  {/if}
  <div class="hv-section-card-body px-5 pb-5 {hasHeader ? (divider ? 'pt-3' : 'pt-1') : 'pt-5'}">
    {@render children()}
  </div>
</section>