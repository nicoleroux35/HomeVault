<script lang="ts">
  /**
   * Graphical screen for an ISO-install VM. noVNC (`@novnc/novnc`) connected to
   * GET /api/vm/screen/ws — the backend relays the browser to the VM's
   * loopback VNC server (added via `raw.qemu -vnc 127.0.0.1:…`). Single-use
   * ticket, admin-only.
   */
  import { onMount, onDestroy } from 'svelte';
  import RFB from '@novnc/novnc';
  import { IconX, IconRefresh, IconMaximize, IconKeyboard } from '@tabler/icons-svelte';
  import { t, translate } from '$lib/stores/i18n';

  interface Props { vmName: string; onClose: () => void }
  let { vmName, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let rfb: RFB | null = null;
  let destroyed = false;
  let status = $state<'connecting' | 'connected' | 'closed' | 'error'>('connecting');
  let detail = $state('');

  async function ticket(): Promise<string | null> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    const st = localStorage.getItem('session-token');
    const ct = localStorage.getItem('csrf-token');
    if (st) headers['X-Session-Token'] = st;
    if (ct) headers['X-CSRF-Token'] = ct;
    try {
      const res = await fetch(`/api/vm/${encodeURIComponent(vmName)}/screen/ticket`, {
        method: 'POST',
        headers,
      });
      const body = await res.json();
      if (body?.success && body?.data?.ticket) return body.data.ticket as string;
      detail = body?.message || translate('vm.screen_failed');
      return null;
    } catch (e) {
      detail = e instanceof Error ? e.message : 'network error';
      return null;
    }
  }

  async function connect() {
    if (destroyed || !host) return;
    status = 'connecting';
    detail = '';
    const tk = await ticket();
    if (destroyed) return;
    if (!tk) {
      status = 'error';
      return;
    }
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${window.location.host}/api/vm/screen/ws?ticket=${encodeURIComponent(tk)}`;
    try {
      rfb = new RFB(host, url, { wsProtocols: ['binary'] });
      rfb.scaleViewport = true;
      rfb.clipViewport = true;
      rfb.background = '#0b0e14';
      rfb.addEventListener('connect', () => { status = 'connected'; });
      rfb.addEventListener('disconnect', (e: Event) => {
        if (destroyed) return;
        const clean = (e as CustomEvent).detail?.clean;
        status = clean ? 'closed' : 'error';
        if (!clean) detail = translate('vm.screen_lost');
      });
      rfb.addEventListener('securityfailure', (e: Event) => {
        status = 'error';
        detail = (e as CustomEvent).detail?.reason || '';
      });
    } catch (e) {
      status = 'error';
      detail = e instanceof Error ? e.message : String(e);
    }
  }

  function reconnect() {
    try { rfb?.disconnect(); } catch { /* ignore */ }
    rfb = null;
    connect();
  }
  function ctrlAltDel() { try { rfb?.sendCtrlAltDel(); } catch { /* ignore */ } }
  function fullscreen() {
    const el = host?.parentElement;
    if (el && !document.fullscreenElement) el.requestFullscreen?.();
    else document.exitFullscreen?.();
  }

  onMount(connect);
  onDestroy(() => {
    destroyed = true;
    try { rfb?.disconnect(); } catch { /* ignore */ }
  });

  // Portal over the desktop window's transform (see VmConsole).
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }
</script>

<div class="scr-backdrop" use:portal role="dialog" aria-modal="true" aria-label={translate('vm.screen_title', { name: vmName })}>
  <div class="scr-window">
    <header class="scr-bar">
      <div class="scr-title">
        <span class="scr-dot" class:ok={status === 'connected'} class:bad={status === 'error' || status === 'closed'}></span>
        {$t('vm.screen_title', { name: vmName })}
      </div>
      <div class="scr-actions">
        <button class="scr-btn" onclick={ctrlAltDel} disabled={status !== 'connected'} title="Ctrl+Alt+Del">
          <IconKeyboard size={15} stroke={2} /> Ctrl+Alt+Del
        </button>
        <button class="scr-btn" onclick={fullscreen} aria-label={$t('vm.screen_fullscreen')}><IconMaximize size={16} stroke={2} /></button>
        {#if status === 'closed' || status === 'error'}
          <button class="scr-btn" onclick={reconnect}><IconRefresh size={15} stroke={2} /> {$t('vm.console_reconnect')}</button>
        {/if}
        <button class="scr-btn scr-btn-close" onclick={onClose} aria-label={$t('common.close')}><IconX size={18} stroke={2} /></button>
      </div>
    </header>

    <div class="scr-stage">
      <div class="scr-host" bind:this={host}></div>
      {#if status !== 'connected'}
        <div class="scr-overlay">
          {#if status === 'connecting'}
            <div class="scr-spin"></div>
            <p>{$t('vm.screen_connecting')}</p>
          {:else if status === 'error'}
            <p class="scr-err">{$t('vm.screen_failed')}</p>
            {#if detail}<p class="scr-detail">{detail}</p>{/if}
          {:else}
            <p>{$t('vm.screen_closed')}</p>
          {/if}
        </div>
      {/if}
    </div>

    <p class="scr-hint">{$t('vm.screen_hint')}</p>
  </div>
</div>

<style>
  .scr-backdrop {
    position: fixed; inset: 0; z-index: 200; display: flex;
    align-items: center; justify-content: center; padding: 2vh 2vw;
    background: rgba(11, 14, 20, 0.6);
  }
  .scr-window {
    width: min(1280px, 100%); height: min(860px, 96vh);
    display: flex; flex-direction: column;
    border-radius: 16px; overflow: hidden; background: #0b0e14;
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: 0 30px 80px rgba(0, 0, 0, 0.5);
  }
  .scr-bar {
    display: flex; align-items: center; justify-content: space-between; gap: var(--hv-space-2-5);
    padding: var(--hv-space-2) var(--hv-space-3); background: #12161f; flex: none;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }
  .scr-title { display: flex; align-items: center; gap: var(--hv-space-2); color: #d7dae0; font-size: var(--hv-text-base); font-weight: var(--hv-font-weight-semibold); }
  .scr-dot { width: 9px; height: 9px; border-radius: 50%; background: var(--hv-warning); flex: none; }
  .scr-dot.ok { background: var(--hv-success); }
  .scr-dot.bad { background: var(--hv-error); }
  .scr-actions { display: flex; align-items: center; gap: var(--hv-space-2); flex-wrap: wrap; justify-content: flex-end; }
  .scr-btn {
    display: inline-flex; align-items: center; gap: 6px; height: 31px; padding: 0 11px;
    border-radius: 8px; border: 1px solid rgba(255, 255, 255, 0.16);
    background: rgba(255, 255, 255, 0.06); color: #d7dae0;
    font-family: inherit; font-size: var(--hv-text-sm); font-weight: var(--hv-font-weight-medium); cursor: pointer; white-space: nowrap;
  }
  .scr-btn:hover:not(:disabled) { background: rgba(255, 255, 255, 0.12); }
  .scr-btn:disabled { opacity: 0.4; cursor: default; }
  .scr-btn-close { padding: 0 8px; }

  .scr-stage { flex: 1; min-height: 0; position: relative; background: #0b0e14; display: flex; }
  .scr-host { flex: 1; min-width: 0; min-height: 0; }
  .scr-host :global(canvas) { display: block; margin: auto; }
  .scr-overlay {
    position: absolute; inset: 0; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 12px;
    color: #9aa4b2; font-size: var(--hv-text-base); background: rgba(11, 14, 20, 0.75);
  }
  .scr-err { color: var(--hv-error); font-weight: var(--hv-font-weight-semibold); }
  .scr-detail { font-size: var(--hv-text-xs); color: #7d8694; max-width: 60ch; text-align: center; }
  .scr-spin {
    width: 30px; height: 30px; border-radius: 50%;
    border: 3px solid rgba(255, 255, 255, 0.15); border-top-color: var(--hv-primary);
    animation: scr-spin 0.8s linear infinite;
  }
  @keyframes scr-spin { to { transform: rotate(360deg); } }
  .scr-hint {
    margin: 0; padding: var(--hv-space-2) var(--hv-space-3); font-size: var(--hv-text-xs); color: #7d8694;
    background: #12161f; border-top: 1px solid rgba(255, 255, 255, 0.06); flex: none;
  }
</style>
