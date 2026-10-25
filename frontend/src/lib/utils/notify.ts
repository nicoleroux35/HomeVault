/* =============================================================================
 * File          : notify.ts
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One way to surface an action failure or a silent success (ROADMAP X.3).
 *   Mutating handlers used to swallow their failure in an empty `else {}`,
 *   so a refused click looked like a click that did nothing.
 *
 * Dependencies:
 *   - svelte-sonner (the app-root toaster, see lib/components/theme/Toast.svelte)
 *   - lib/stores/i18n for the translated fallback
 *
 * Usage:
 *   `import { notifyError, notifySuccess } from '$lib/utils/notify';`
 *   `if (!res.success) notifyError(res);`
 * =============================================================================
 */

import { toast } from 'svelte-sonner';
import { translate } from '$lib/stores/i18n';

/** The shape every `api.*` call resolves to (`lib/types/api.ts`). */
export interface ActionResponse {
  success?: boolean;
  message?: string;
  error?: string;
}

/**
 * Surface an action failure in the app toast (ROADMAP X.3).
 *
 * The server's own reason is what makes the failure actionable, so it is shown
 * verbatim (`message`, then `error`); the translated fallback only appears when
 * the server gave neither — a transport failure or a bare `{ success: false }`.
 */
export function notifyError(
  response?: ActionResponse | null,
  fallbackKey = 'common.action_failed',
): void {
  const reason = response?.message || response?.error || translate(fallbackKey);
  toast.error(reason);
}

/**
 * Positive confirmation for an action whose success would otherwise leave no
 * visible trace (e.g. saving a protocol's configuration, which only rewrites a
 * root-owned file). Actions that visibly reload a list do not need this.
 */
export function notifySuccess(
  messageKey: string,
  vars?: Record<string, string>,
): void {
  toast.success(translate(messageKey, vars));
}
