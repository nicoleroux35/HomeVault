<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM - Alert Component
   * Version: 5.0 - wraps the real shadcn-svelte Alert (see
   * components/ui/alert). External prop API unchanged (replaces the 4.0 Web
   * Awesome wrapper 2026-09-11).
   *
   * `variant` maps 1:1 to shadcn alert variants (the semantic tints come
   * from the `--hv-*` tokens via app.css's @theme bridge). The leading icon
   * keeps the HomeVault Tabler voice (per-variant `--hv-*` color); `actions`
   * render in the alert-action slot; `dismissible` renders a ghost close
   * button wired to `ondismiss`.
   */
  import { IconInfoCircle, IconCircleCheck, IconAlertTriangle, IconAlertCircle, IconX } from '@tabler/icons-svelte';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { t } from '$lib/stores/i18n';
  import type { Snippet } from 'svelte';

  let {
    variant = 'info',
    title = '',
    dismissible = false,
    icon = true,
    children,
    actions,
    ondismiss
  }: {
    variant?: 'info' | 'success' | 'warning' | 'error';
    title?: string;
    dismissible?: boolean;
    icon?: boolean;
    children?: Snippet;
    actions?: Snippet;
    ondismiss?: () => void;
  } = $props();

  const icons = {
    info: IconInfoCircle,
    success: IconCircleCheck,
    warning: IconAlertTriangle,
    error: IconAlertCircle
  };
  const shadcnVariant: Record<typeof variant, 'info' | 'success' | 'warning' | 'error'> = {
    info: 'info',
    success: 'success',
    warning: 'warning',
    error: 'error',
  };
  const iconColors = {
    info: 'var(--hv-primary)',
    success: 'var(--hv-success)',
    warning: 'var(--hv-warning)',
    error: 'var(--hv-error)',
  };

  const IconComponent = $derived(icons[variant]);
</script>

<Alert.Root variant={shadcnVariant[variant]} class="hv-alert hv-alert-{variant}">
  {#if icon}
    <IconComponent size={20} color={iconColors[variant]} />
  {/if}

  {#if title}
    <Alert.Title>{title}</Alert.Title>
  {/if}
  <Alert.Description>
    {@render children?.()}
  </Alert.Description>

  {#if actions}
    <Alert.Action>
      {@render actions()}
    </Alert.Action>
  {/if}

  {#if dismissible}
    <Alert.Action>
      <Button variant="ghost" size="xs" onclick={ondismiss} aria-label={$t('common.close')}>
        <IconX size={18} />
      </Button>
    </Alert.Action>
  {/if}
</Alert.Root>