<!-- ========================================================================
     File          : VmConsole.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Shell console for one VM: WebSocket to the backend's incus-exec relay
       using the Terminal app's wire protocol (binary frames = raw bytes,
       JSON text frame = resize), with a single-use ticket.

     Dependencies:
       - svelte (>= 5), $lib/services/api

     Usage:
       Opened from the Virtual Machines app.
     ======================================================================== -->
<script lang="ts">
  /**
   * Full-screen-ish shell console for one machine. Talks to
   * GET /api/vm/console/ws (an interactive `incus exec` login shell relayed
   * by the backend). Same wire protocol as the Terminal app: binary frames
   * are raw bytes, a JSON `{cols,rows}` text frame is a resize.
   */
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from 'xterm';
  import { FitAddon } from 'xterm-addon-fit';
  import 'xterm/css/xterm.css';
  import { IconX, IconRefresh } from '@tabler/icons-svelte';
  import { t, translate } from '$lib/stores/i18n';

  interface Props {
    vmName: string;
    onClose: () => void;
  }
  let { vmName, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let ws: WebSocket | null = null;
  let ro: ResizeObserver | null = null;
  let destroyed = false;
  let status = $state<'connecting' | 'connected' | 'closed' | 'error'>('connecting');

  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function authedFetch(method: string, endpoint: string, body?: unknown) {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    const st = localStorage.getItem('session-token');
    const ct = localStorage.getItem('csrf-token');
    if (st) headers['X-Session-Token'] = st;
    if (ct) headers['X-CSRF-Token'] = ct;
    const res = await fetch('/api' + endpoint, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    return res.json();
  }

  function sendResize() {
    if (ws?.readyState === WebSocket.OPEN && term) {
      ws.send(JSON.stringify({ cols: term.cols, rows: term.rows }));
    }
  }

  async function connect() {
    if (destroyed || !term) return;
    status = 'connecting';
    term.writeln(`\x1b[90m${translate('vm.console_connecting')}\x1b[0m`);

    const ticketRes = await authedFetch('POST', '/vm/console/ticket', {
      name: vmName,
      cols: term.cols,
      rows: term.rows,
    });
    if (destroyed) return;
    if (!ticketRes?.success || !ticketRes?.data?.ticket) {
      status = 'error';
      term.writeln(`\r\n\x1b[31m${ticketRes?.message || translate('vm.console_failed')}\x1b[0m`);
      return;
    }

    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${window.location.host}/api/vm/console/ws?ticket=${encodeURIComponent(ticketRes.data.ticket)}`;
    ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      status = 'connected';
      sendResize();
      term?.focus();
    };
    ws.onmessage = (ev) => {
      if (!term) return;
      if (ev.data instanceof ArrayBuffer) term.write(new Uint8Array(ev.data));
      else term.write(String(ev.data));
    };
    ws.onerror = () => {
      status = 'error';
    };
    ws.onclose = () => {
      if (destroyed) return;
      status = 'closed';
      term?.writeln(`\r\n\x1b[90m${translate('vm.console_closed')}\x1b[0m`);
    };
  }

  function reconnect() {
    try { ws?.close(); } catch { /* ignore */ }
    ws = null;
    term?.clear();
    connect();
  }

  onMount(() => {
    term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: 'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace',
      theme: { background: '#0b0e14', foreground: '#d7dae0', cursor: '#4d9fff' },
      scrollback: 5000,
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    if (host) {
      term.open(host);
      fit.fit();
    }
    term.onData((d) => {
      if (ws?.readyState === WebSocket.OPEN) ws.send(encoder.encode(d));
    });
    ro = new ResizeObserver(() => {
      try { fit?.fit(); sendResize(); } catch { /* ignore */ }
    });
    if (host) ro.observe(host);
    connect();
  });

  onDestroy(() => {
    destroyed = true;
    ro?.disconnect();
    try { ws?.close(); } catch { /* ignore */ }
    term?.dispose();
  });

  // The desktop window frame has a `transform`, which turns every
  // `position: fixed` descendant into a window-relative (and clipped) node.
  // Portal the overlay to <body> so it truly covers the viewport.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }
</script>

<div class="con-backdrop" use:portal role="dialog" aria-modal="true" aria-label={translate('vm.console_title', { name: vmName })}>
  <div class="con-window">
    <header class="con-bar">
      <div class="con-title">
        <span class="con-dot" class:ok={status === 'connected'} class:bad={status === 'error' || status === 'closed'}></span>
        {$t('vm.console_title', { name: vmName })}
      </div>
      <div class="con-actions">
        {#if status === 'closed' || status === 'error'}
          <button class="con-btn" onclick={reconnect}><IconRefresh size={16} stroke={2} /> {$t('vm.console_reconnect')}</button>
        {/if}
        <button class="con-btn con-btn-close" onclick={onClose} aria-label={$t('common.close')}><IconX size={18} stroke={2} /></button>
      </div>
    </header>
    <div class="con-term" bind:this={host}></div>
    <p class="con-hint">{$t('vm.console_hint')}</p>
  </div>
</div>

<style>
  .con-backdrop {
    position: fixed; inset: 0; z-index: 200; display: flex;
    align-items: center; justify-content: center; padding: 3vh 3vw;
    background: rgba(11, 14, 20, 0.55);
  }
  .con-window {
    width: min(1100px, 100%); height: min(720px, 94vh);
    display: flex; flex-direction: column;
    border-radius: var(--hv-radius-2xl); overflow: hidden; background: #0b0e14;
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: 0 30px 80px rgba(0, 0, 0, 0.5);
  }
  .con-bar {
    display: flex; align-items: center; justify-content: space-between;
    padding: var(--hv-space-2-5) var(--hv-space-3); background: #12161f;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08); flex: none;
  }
  .con-title { display: flex; align-items: center; gap: var(--hv-space-2-5); color: #d7dae0; font-size: var(--hv-text-base); font-weight: var(--hv-font-weight-semibold); }
  .con-dot { width: 9px; height: 9px; border-radius: 50%; background: var(--hv-warning); }
  .con-dot.ok { background: var(--hv-success); }
  .con-dot.bad { background: var(--hv-error); }
  .con-actions { display: flex; align-items: center; gap: 8px; }
  .con-btn {
    display: inline-flex; align-items: center; gap: 6px; height: 32px; padding: 0 12px;
    border-radius: var(--hv-radius-control); border: 1px solid rgba(255, 255, 255, 0.16);
    background: rgba(255, 255, 255, 0.06); color: #d7dae0;
    font-family: inherit; font-size: var(--hv-text-sm); font-weight: var(--hv-font-weight-medium); cursor: pointer;
  }
  .con-btn:hover { background: rgba(255, 255, 255, 0.12); }
  .con-btn-close { padding: 0 8px; }
  .con-term { flex: 1; min-height: 0; padding: var(--hv-space-2) var(--hv-space-1) var(--hv-space-1) var(--hv-space-2-5); background: #0b0e14; }
  .con-term :global(.xterm) { height: 100%; }
  .con-hint {
    margin: 0; padding: var(--hv-space-2) var(--hv-space-3); font-size: var(--hv-text-xs); color: #7d8694;
    background: #12161f; border-top: 1px solid rgba(255, 255, 255, 0.06); flex: none;
  }
</style>
