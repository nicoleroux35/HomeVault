<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Button Component
   * Version: 5.0 - wraps the real shadcn-svelte Button (Melt/bits-ui
   * primitive, see components/ui/button). External prop API unchanged
   * (replaces the 4.0 Web Awesome wrapper 2026-09-11; no call site needed
   * updating).
   *
   * `variant` maps 1:1 to shadcn button variants — primary→default,
   * secondary→secondary, ghost→ghost, danger→danger, success→success,
   * warning→warning (the semantic fills use the HomeVault `--hv-*` tokens via
   * app.css's @theme bridge). `loading` renders the shared Spinner in place
   * of the children and disables the button, matching the old behavior.
   */

  import { Button as ShadcnButton } from '$lib/components/ui/button/index.js';
  import Spinner from './Spinner.svelte';

  let {
    variant = 'primary',
    size = 'md',
    icon = '',
    disabled = false,
    loading = false,
    type = 'button',
    fullWidth = false,
    class: extraClass = '',
    'data-testid': testId,
    onclick,
    onmouseenter,
    onmouseleave,
    onfocus,
    onblur,
    children,
    ...rest
  }: {
    variant?: 'primary' | 'secondary' | 'success' | 'danger' | 'ghost' | 'warning';
    size?: 'sm' | 'md' | 'lg';
    disabled?: boolean;
    loading?: boolean;
    type?: 'button' | 'submit' | 'reset';
    icon?: string;
    fullWidth?: boolean;
    /** Extra class(es) merged onto the rendered button, e.g. for a caller's
     * own layout hook (`flex:1`, a page-scoped `:global()` rule). Always
     * appended after the internal hv-btn-* classes. */
    class?: string;
    /** Test hook forwarded to the rendered button. */
    'data-testid'?: string;
    onclick?: (e: MouseEvent) => void;
    onmouseenter?: (e: MouseEvent) => void;
    onmouseleave?: (e: MouseEvent) => void;
    onfocus?: (e: FocusEvent) => void;
    onblur?: (e: FocusEvent) => void;
    children?: import('svelte').Snippet;
  } = $props();

  const variantMap: Record<typeof variant, 'default' | 'secondary' | 'success' | 'danger' | 'warning' | 'ghost'> = {
    primary: 'default',
    secondary: 'secondary',
    success: 'success',
    danger: 'danger',
    warning: 'warning',
    ghost: 'ghost',
  };
  const sizeMap: Record<'sm' | 'md' | 'lg', 'sm' | 'default' | 'lg'> = {
    sm: 'sm',
    md: 'default',
    lg: 'lg',
  };

  let classes = $derived([
    'hv-btn',
    `hv-btn-${variant}`,
    icon ? 'hv-btn-icon' : '',
    fullWidth ? 'hv-w-full' : '',
    extraClass,
  ].filter(Boolean).join(' '));
</script>

<ShadcnButton
  {type}
  variant={variantMap[variant]}
  size={sizeMap[size]}
  disabled={disabled || loading || undefined}
  class={classes}
  data-testid={testId}
  {onclick}
  {onmouseenter}
  {onmouseleave}
  {onfocus}
  {onblur}
  {...rest}
>
  {#if loading}
    <Spinner size="sm" />
  {:else}
    {@render children?.()}
  {/if}
</ShadcnButton>

<style>
  :global(button.hv-w-full) {
    width: 100%;
  }
  :global(button.hv-btn-icon) {
    width: var(--hv-button-height);
    padding: 0;
  }
  :global(button.hv-btn-icon.hv-btn-sm) {
    width: 36px;
  }
</style>