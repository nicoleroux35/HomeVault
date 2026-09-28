/**
 * HOMEVAULT — focusTrap
 *
 * Svelte `use:focusTrap` action for the hand-rolled overlay/dialog elements
 * that are NOT built on bits-ui Dialog (which already traps focus): the
 * Desktop power/confirm modals and the Launchpad. Gives them the WCAG 2.2
 * keyboard behaviour of a modal dialog:
 *
 *  - moves focus into the dialog when it opens (first focusable control),
 *  - keeps Tab/Shift+Tab cycling inside the dialog (no keyboard trap, but
 *    also no escape from the modal — WCAG 2.1.1 / 2.1.2),
 *  - closes on Escape via the `escape` callback,
 *  - restores focus to the element that had it before the dialog opened
 *    (WCAG 2.4.3), so the keyboard user lands back on the trigger button.
 *
 * Usage:
 *   <div role="dialog" aria-modal="true" use:focusTrap={{ escape: close }}>
 *
 * The node should have `tabindex="-1"` (or the first control will receive
 * focus). Do not use on bits-ui Dialog content — it manages its own focus.
 */

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), ' +
  'textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [role="button"]:not([tabindex="-1"])';

function focusables(node: HTMLElement): HTMLElement[] {
  return Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.offsetParent !== null || el === document.activeElement,
  );
}

export function focusTrap(
  node: HTMLElement,
  opts: { escape?: () => void } = {},
): { destroy: () => void } {
  const previouslyFocused = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      opts.escape?.();
      return;
    }
    if (e.key !== 'Tab') return;

    const list = focusables(node);
    if (list.length === 0) {
      e.preventDefault();
      node.focus();
      return;
    }
    const first = list[0];
    const last = list[list.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  // Focus the first control once the dialog is in the DOM. Synchronous (not
  // rAF) so a component that auto-focuses a specific field in onMount (e.g.
  // the Launchpad's search box) can run afterwards and win; the trap's job is
  // only to guarantee focus starts inside the dialog. If focus already landed
  // inside (reused node), leave it alone.
  if (!node.contains(document.activeElement)) {
    const list = focusables(node);
    (list[0] ?? node).focus();
  }

  node.addEventListener('keydown', onKeydown);

  return {
    destroy() {
      node.removeEventListener('keydown', onKeydown);
      previouslyFocused?.focus();
    },
  };
}