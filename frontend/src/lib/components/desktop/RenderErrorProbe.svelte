<script lang="ts">
  /**
   * RenderErrorProbe — test-only companion of WindowFrame's `<svelte:boundary>`
   * (UI audit W10).
   *
   * Renders nothing in normal operation. When WindowFrame arms it (the app was
   * opened with `?hv-render-error`), it throws during its first render so the
   * boundary's fallback can be exercised end to end: assert the fallback copy,
   * then click Retry — WindowFrame disarms the probe, `reset()` re-renders the
   * children and the real route content comes back.
   *
   * Deliberately not gated on `import.meta.env.DEV`: E2E runs against the
   * production bundle served on :8080, and a query-param trigger cannot fire
   * unless the URL asks for it.
   */
  throw new Error('RenderErrorProbe: intentional render failure (?hv-render-error)');
</script>
