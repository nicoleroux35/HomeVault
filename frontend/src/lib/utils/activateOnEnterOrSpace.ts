/**
 * activateOnEnterOrSpace — one shared keyboard-activation helper for
 * `role="button"` containers (UI audit W8/W18).
 *
 * A non-native element carrying `role="button"` + `tabindex="0"` must activate
 * on BOTH Enter and Space (WAI-ARIA button pattern). Space additionally needs
 * `preventDefault()`, otherwise the page scrolls while the row opens.
 *
 * Usage in a Svelte template:
 *
 *   onkeydown={(e) => activateOnEnterOrSpace(e, () => openDetail(m.name))}
 *
 * Returns `true` when the event was consumed (Enter or Space), so a caller can
 * chain extra logic:
 *
 *   if (activateOnEnterOrSpace(e, fn)) return;
 */
export function activateOnEnterOrSpace(e: KeyboardEvent, activate: () => void): boolean {
  // Ignore key repeats: holding Enter/Space must not re-fire the action.
  if (e.repeat) return false;

  if (e.key === 'Enter') {
    e.preventDefault();
    activate();
    return true;
  }

  if (e.key === ' ' || e.key === 'Spacebar') {
    // Space would scroll the nearest scrollable ancestor on a role="button".
    e.preventDefault();
    activate();
    return true;
  }

  return false;
}
