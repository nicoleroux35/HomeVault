<script lang="ts">
  import { api } from '$lib/services/api';

  export let username: string;
  export let size: number = 36;
  export let seed: string | null | undefined = null;

  // We append a cache-buster when the seed changes so the browser refetches
  // the freshly generated SVG (the backend sends Cache-Control: public,
  // max-age=300, immutable for stored avatars).
  $: avatarUrl = api.userAvatarUrl(username, size) + (seed ? `&_=${encodeURIComponent(seed)}` : '');
</script>

<img
  class="hv-user-avatar"
  src={avatarUrl}
  alt={username}
  width={size}
  height={size}
  on:error={(e) => {
    // Graceful fallback to a neutral SVG so we never break the layout.
    (e.currentTarget as HTMLImageElement).src =
      'data:image/svg+xml;utf8,' +
      encodeURIComponent(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="var(--hv-border)"/><circle cx="32" cy="24" r="10" fill="var(--hv-neutral-400)"/><path d="M12 56c0-11 9-20 20-20s20 9 20 20z" fill="var(--hv-neutral-400)"/></svg>'
      );
  }}
/>

<style>
  .hv-user-avatar {
    border-radius: var(--hv-radius-full);
    object-fit: cover;
    flex-shrink: 0;
    background: var(--hv-neutral-100, var(--hv-neutral-100));
    border: 1px solid var(--hv-border, var(--hv-border));
  }
</style>