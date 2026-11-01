<!-- ========================================================================
     File          : PublicShare.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.1.0

     Description:
       Public landing page of a share link (/share/<token>): asks for the
       password only when the link has one, then hands the download to the
       browser instead of buffering it into a JavaScript blob.

     Dependencies:
       - svelte (>= 5), $lib/stores/i18n, $lib/components/theme

     Usage:
       Rendered by App.svelte for any path under /share/.
     ======================================================================== -->
<script lang="ts">
  /**
   * Public share landing page.
   *
   * Two invariants worth keeping:
   * - **The password never enters a URL.** It is posted once to
   *   `/share/{token}/unlock`, which answers with a short-lived, path-scoped
   *   HttpOnly cookie; the download itself is then an ordinary navigation, so
   *   ranges, resume and multi-gigabyte files behave like any other download
   *   instead of being loaded into memory.
   * - **Ask only what is needed.** A `HEAD` probe (which never spends a
   *   download slot) decides between "press download" and "enter the
   *   password", so an open link never shows a password field it does not
   *   need and a spent link says so before the visitor fills a form.
   */
  import { onMount } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import { Button } from '$lib/components/theme';

  /** What the link asked for, as reported by the probe. */
  type Gate = 'checking' | 'ready' | 'password' | 'session' | 'unavailable' | 'error';

  const token = window.location.pathname.split('/').filter(Boolean).at(-1) ?? '';
  const shareUrl = `/share/${encodeURIComponent(token)}`;

  let gate = $state<Gate>('checking');
  let password = $state('');
  let error = $state('');
  let busy = $state(false);

  onMount(probe);

  async function probe() {
    gate = 'checking';
    error = '';
    try {
      const response = await fetch(shareUrl, { method: 'HEAD' });
      if (response.ok) {
        gate = 'ready';
        return;
      }
      const reason = response.headers.get('x-hv-share-auth');
      if (response.status === 401 && reason === 'session') {
        gate = 'session';
      } else if (response.status === 401) {
        gate = 'password';
      } else {
        gate = 'unavailable';
      }
    } catch {
      gate = 'error';
    }
  }

  /**
   * Let the browser do the download. It already holds the unlock cookie, so a
   * plain navigation streams the file to disk and supports `Range`/resume —
   * something a `fetch` + blob cannot do without holding the whole file.
   */
  function startDownload() {
    const anchor = document.createElement('a');
    anchor.href = shareUrl;
    anchor.rel = 'noopener';
    anchor.download = '';
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
  }

  async function unlock(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    error = '';
    try {
      const response = await fetch(`${shareUrl}/unlock`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ password }),
      });
      if (response.ok) {
        startDownload();
        return;
      }
      if (response.status === 429) {
        error = $t('explorer.share_public_throttled');
      } else if (response.status === 401) {
        error = $t('explorer.share_public_wrong_password');
      } else {
        gate = 'unavailable';
      }
    } catch {
      error = $t('explorer.share_public_unavailable');
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head>
  <title>{$t('explorer.share_public_title')}</title>
  <meta name="robots" content="noindex, nofollow" />
</svelte:head>

<main class="min-h-screen grid place-items-center bg-[var(--hv-background)] p-4">
  <section
    aria-labelledby="public-share-heading"
    aria-busy={gate === 'checking'}
    class="w-full max-w-md bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] rounded-2xl p-6"
  >
    <h1 id="public-share-heading" class="m-0 mb-4 text-t-title-1 text-[var(--hv-shell-text-strong)]">
      {$t('explorer.share_public_title')}
    </h1>

    {#if gate === 'checking'}
      <p role="status" class="m-0 text-t-body text-[var(--hv-shell-text-2)]">
        {$t('explorer.share_public_checking')}
      </p>
    {:else if gate === 'ready'}
      <p class="m-0 mb-4 text-t-body text-[var(--hv-shell-text-2)]">
        {$t('explorer.share_public_ready')}
      </p>
      <Button variant="primary" onclick={startDownload}>{$t('explorer.share_public_download')}</Button>
    {:else if gate === 'password'}
      <form onsubmit={unlock} class="grid gap-4">
        <label class="grid gap-1 text-t-body text-[var(--hv-shell-text-strong)]" for="public-share-password">
          {$t('explorer.share_public_password')}
          <input
            id="public-share-password"
            type="password"
            autocomplete="current-password"
            required
            class="h-11 px-3 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-glass)] text-[var(--hv-shell-text-strong)]"
            bind:value={password}
          />
        </label>
        {#if error}<p role="alert" class="m-0 text-t-body text-[var(--hv-error)]">{error}</p>{/if}
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? $t('explorer.share_public_downloading') : $t('explorer.share_public_download')}
        </Button>
      </form>
    {:else if gate === 'session'}
      <p role="alert" class="m-0 text-t-body text-[var(--hv-shell-text-2)]">
        {$t('explorer.share_public_session_required')}
      </p>
      <a
        href="/login"
        class="mt-4 inline-flex items-center min-h-11 text-t-body font-semibold text-[var(--hv-primary)] underline underline-offset-2"
      >
        {$t('explorer.share_public_sign_in')}
      </a>
    {:else}
      <p role="alert" class="m-0 text-t-body text-[var(--hv-error)]">
        {$t('explorer.share_public_unavailable')}
      </p>
      <Button variant="secondary" class="mt-4" onclick={probe}>{$t('explorer.share_public_retry')}</Button>
    {/if}
  </section>
</main>