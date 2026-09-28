/**
 * Keep an app window's data fresh without a manual "Refresh" button.
 *
 * `onAppVisible(fn)` re-runs `fn` whenever the tab/window regains attention —
 * it becomes visible again after being hidden, or the browser window regains
 * focus. A short cooldown coalesces the `visibilitychange` + `focus` pair that
 * fires together when a user comes back; longer-term de-duplication is handled
 * by the `api.ts` response cache (5s TTL), so a cache hit costs nothing.
 *
 * Pass `pollMs` for the genuinely live screens (Docker, Health): while the
 * window is visible it also re-runs `fn` on that slow interval.
 *
 * Returns a cleanup function — call it from the component's `onMount` return,
 * `onDestroy`, or as an `$effect` teardown.
 *
 * The `visibilitychange` half mirrors the pattern already used in
 * Storage.svelte / Explorer.svelte / Desktop.svelte.
 */
export function onAppVisible(
  fn: () => void,
  opts: { cooldownMs?: number; pollMs?: number } = {},
): () => void {
  const cooldown = opts.cooldownMs ?? 1_500;
  let cooldownUntil = 0;
  let pollId: ReturnType<typeof setInterval> | undefined;

  const hidden = () => typeof document !== 'undefined' && document.hidden;

  const run = () => {
    if (hidden()) return;
    const now = Date.now();
    if (now < cooldownUntil) return;
    cooldownUntil = now + cooldown;
    fn();
  };

  const onVisibility = () => { if (!hidden()) run(); };

  document.addEventListener('visibilitychange', onVisibility);
  window.addEventListener('focus', run);

  if (opts.pollMs && opts.pollMs > 0) {
    pollId = setInterval(() => { if (!hidden()) fn(); }, opts.pollMs);
  }

  return () => {
    document.removeEventListener('visibilitychange', onVisibility);
    window.removeEventListener('focus', run);
    if (pollId) clearInterval(pollId);
  };
}
