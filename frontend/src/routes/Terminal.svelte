<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { Terminal } from 'xterm';
  import { FitAddon } from 'xterm-addon-fit';
  import { WebLinksAddon } from 'xterm-addon-web-links';
  import 'xterm/css/xterm.css';
  import { IconTerminal as TerminalIcon, IconDeviceDesktop, IconPlus, IconX } from '@tabler/icons-svelte';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { authStore } from '$lib/stores/auth';

  /** user@host shown in the info bar, mirroring the design template. */
  const username = $derived($authStore.user?.username || 'user');
  const hostname = typeof window !== 'undefined' ? window.location.hostname : 'homevault';

  /** Hard cap, mirrors MAX_TABS_PER_USER in src/handlers/terminal_pty.rs. */
  const MAX_TABS = 8;

  type Tab = {
    id: string;
    title: string;
    container: HTMLDivElement | null;
    terminal: Terminal | null;
    fitAddon: FitAddon | null;
    ws: WebSocket | null;
    connected: boolean;
    started: boolean;
    reconnectTimer: ReturnType<typeof setTimeout> | null;
    resizeObserver: ResizeObserver | null;
    // --- completion state (per shell) ---
    inputLine: string;
    cwd: string;
    candidates: CompletionCandidate[];
    ghost: { row: number; col: number; text: string } | null;
    ghostSuffix: string;
    compTimer: ReturnType<typeof setTimeout> | null;
    cwdTimer: ReturnType<typeof setTimeout> | null;
  };

  interface CompletionCandidate {
    name: string;
    isDir: boolean;
  }

  /** The token the user is currently typing (last whitespace-delimited word). */
  function parseCurrentToken(line: string): { token: string; tokenStart: number } {
    let tokenStart = 0;
    let inQuote = false;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (c === '"' || c === "'") inQuote = !inQuote;
      else if (c === ' ' && !inQuote) tokenStart = i + 1;
    }
    return { token: line.slice(tokenStart), tokenStart };
  }

  /** Common shell commands offered for first-token completion. The shell's
   *  own Tab completion stays authoritative for anything not in this list. */
  const COMMANDS = [
    'alias', 'apt', 'apt-get', 'bash', 'btrfs', 'cargo', 'cat', 'cd', 'chmod', 'chown',
    'clear', 'cp', 'crontab', 'curl', 'date', 'df', 'diff', 'dig', 'docker', 'du',
    'echo', 'env', 'exit', 'export', 'find', 'free', 'git', 'grep', 'groupadd',
    'head', 'history', 'hostname', 'htop', 'ifconfig', 'ip', 'journalctl', 'kill',
    'killall', 'less', 'ln', 'locale', 'ls', 'lsblk', 'lsof', 'lsusb', 'make',
    'mkdir', 'mktemp', 'more', 'mount', 'mv', 'nano', 'nc', 'netstat', 'nice',
    'nohup', 'openssl', 'passwd', 'ping', 'pkill', 'ps', 'pwd', 'python3', 'rm',
    'rmdir', 'rsync', 'scp', 'sed', 'setquota', 'sh', 'sha256sum', 'sleep', 'smartctl',
    'sort', 'source', 'ssh', 'ssh-keygen', 'ss', 'su', 'sudo', 'sync', 'systemctl',
    'tail', 'tar', 'tee', 'time', 'tmux', 'top', 'touch', 'tr', 'traceroute', 'tree',
    'ufw', 'umount', 'unalias', 'uname', 'unzip', 'uptime', 'useradd', 'usermod',
    'uuidgen', 'vim', 'vi', 'watch', 'wc', 'wget', 'which', 'who', 'whoami', 'xz',
    'yes', 'zfs', 'zpool', 'zypper', '7z', 'dd', 'df', 'mkfs.ext4', 'mkfs.xfs',
    'mdadm', 'blkid', 'fdisk', 'parted', 'wipefs', 'cryptsetup', 'mount.cifs', 'ncdu',
  ];

  let tabs = $state<Tab[]>([]);
  let activeTabId = $state('');
  let destroyed = false;

  /** Completion popup (shown on Tab when several candidates exist). */
  let popupVisible = $state(false);
  let popupCandidates = $state<CompletionCandidate[]>([]);

  /** Remembering the open tabs (ids + titles) lets a reopened Terminal window
   *  reattach to the shells still running server-side — the same
   *  "close window, come back, resume" behaviour the single-session terminal
   *  had, now per tab. Server reaps a tab left unattached past its idle
   *  timeout, so a stale id here just starts a fresh shell. */
  const TABS_STORAGE_KEY = 'hv-terminal-tabs';

  function persistTabs() {
    try {
      localStorage.setItem(
        TABS_STORAGE_KEY,
        JSON.stringify({ activeTabId, tabs: tabs.map((t) => ({ id: t.id, title: t.title })) }),
      );
    } catch {
      /* private window / storage disabled — non-fatal, tabs just won't persist */
    }
  }

  function loadPersistedTabs(): { id: string; title: string }[] | null {
    try {
      const raw = localStorage.getItem(TABS_STORAGE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      const list = Array.isArray(parsed?.tabs) ? parsed.tabs : null;
      if (!list?.length) return null;
      return list
        .filter((t: any) => typeof t?.id === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(t.id))
        .slice(0, MAX_TABS)
        .map((t: any) => ({ id: t.id, title: typeof t.title === 'string' ? t.title : translate('terminal.title') }));
    } catch {
      return null;
    }
  }

  const activeTab = $derived(tabs.find((t) => t.id === activeTabId) ?? null);

  const textEncoder = new TextEncoder();
  const textDecoder = new TextDecoder();

  /** Tab label by position: first tab is just "Terminal", the rest are
   *  "Terminal 2", "Terminal 3", … Titles aren't user-editable, so they're
   *  always derived from the index (including for persisted tabs on reload). */
  function tabTitle(index: number): string {
    const base = translate('terminal.title');
    return index === 0 ? base : `${base} ${index + 1}`;
  }

  function newTabId(): string {
    if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID();
    return 'tab-' + Math.random().toString(36).slice(2, 12);
  }

  function makeTab(title: string): Tab {
    return {
      id: newTabId(),
      title,
      container: null,
      terminal: null,
      fitAddon: null,
      ws: null,
      connected: false,
      started: false,
      reconnectTimer: null,
      resizeObserver: null,
      inputLine: '',
      cwd: '',
      candidates: [],
      ghost: null,
      ghostSuffix: '',
      compTimer: null,
      cwdTimer: null,
    };
  }

  onMount(() => {
    const persisted = loadPersistedTabs();
    if (persisted) {
      tabs = persisted.map((p, i) => {
        const tab = makeTab(tabTitle(i));
        tab.id = p.id;
        return tab;
      });
    } else {
      tabs = [makeTab(tabTitle(0))];
    }
    activateTab(tabs[0].id);
  });

  onDestroy(() => {
    destroyed = true;
    tabs.forEach(cleanupTab);
  });

  function getFontSize(): number {
    if (typeof window === 'undefined') return 14;
    const width = window.innerWidth;
    if (width < 480) return 11;
    if (width < 768) return 12;
    if (width < 1024) return 13;
    return 14;
  }

  function addTab() {
    if (tabs.length >= MAX_TABS) return;
    const tab = makeTab(tabTitle(tabs.length));
    tabs = [...tabs, tab];
    activateTab(tab.id);
    persistTabs();
  }

  async function activateTab(id: string) {
    // A different shell is showing — drop any in-progress completion state.
    const prev = tabs.find((t) => t.id === activeTabId);
    if (prev && prev.id !== id) {
      clearGhost(prev);
      hidePopup();
    }
    activeTabId = id;
    persistTabs();
    await tick();
    const tab = tabs.find((t) => t.id === id);
    if (!tab || !tab.container) return;

    if (!tab.terminal) {
      initTabTerminal(tab);
      connectTab(tab);
    } else if (tab.fitAddon && tab.terminal) {
      tab.terminal.options.fontSize = getFontSize();
      tab.fitAddon.fit();
      tab.terminal.focus();
      sendResize(tab);
    }
  }

  function closeTab(id: string) {
    const tab = tabs.find((t) => t.id === id);
    if (!tab) return;
    const idx = tabs.findIndex((t) => t.id === id);

    cleanupTab(tab);
    api.closeTerminalSession(tab.id).catch(() => {});
    tabs = tabs.filter((t) => t.id !== id);

    if (tabs.length === 0) {
      const fresh = makeTab(translate('terminal.title'));
      tabs = [fresh];
      activateTab(fresh.id);
      return;
    }
    if (activeTabId === id) {
      const next = tabs[Math.max(0, idx - 1)];
      activateTab(next.id);
    }
    persistTabs();
  }

  // xterm paints on a canvas, so its theme needs concrete colours: read the
  // design tokens from :root at init time instead of hard-coding hexes (UI audit W4).
  const cssToken = (name: string) =>
    getComputedStyle(document.documentElement).getPropertyValue(name).trim();

  function initTabTerminal(tab: Tab) {
    const term = new Terminal({
      cursorBlink: true,
      fontSize: getFontSize(),
      fontFamily: "'JetBrains Mono', ui-monospace, monospace",
      theme: {
        background: '#FFFFFF',
        foreground: '#242424',
        cursor: cssToken('--hv-primary'),
        cursorAccent: '#ffffff',
        selectionBackground: 'rgba(15, 108, 189, 0.25)',
        black: '#242424',
        red: '#C50F1F',
        green: '#107C10',
        yellow: '#a86a00',
        blue: cssToken('--hv-primary'),
        magenta: '#7c3aed',
        cyan: '#0e7490',
        white: '#424242',
        brightBlack: '#707070',
        brightRed: '#A20E1A',
        brightGreen: '#0B5A0B',
        brightYellow: '#8F3703',
        brightBlue: '#115EA3',
        brightMagenta: '#6d28d9',
        brightCyan: '#155e75',
        brightWhite: '#242424',
      },
      allowProposedApi: true,
      macOptionIsMeta: true,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(tab.container!);
    fit.fit();

    term.onData((data) => handleTerminalInput(tab, data));

    const ro = new ResizeObserver(() => {
      if (tab.terminal && tab.fitAddon && tab.id === activeTabId) {
        clearGhost(tab);
        hidePopup();
        tab.terminal.options.fontSize = getFontSize();
        tab.fitAddon.fit();
        sendResize(tab);
      }
    });
    ro.observe(tab.container!);

    tab.terminal = term;
    tab.fitAddon = fit;
    tab.resizeObserver = ro;
    tabs = tabs;
  }

  async function connectTab(tab: Tab) {
    if (destroyed) return;
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';

    const ticketRes = await api.getTerminalTicket(tab.id);
    if (destroyed) return;
    if (!ticketRes.success || !ticketRes.data?.ticket) {
      tab.terminal?.writeln(`\r\n\x1b[31m${translate('terminal.not_authenticated')}\x1b[0m\r\n`);
      return;
    }

    const wsUrl = `${protocol}//${window.location.host}/api/terminal/ws?ticket=${encodeURIComponent(ticketRes.data.ticket)}`;
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';
    tab.ws = ws;

    ws.onopen = () => {
      tab.connected = true;
      tab.started = true;
      tabs = tabs;
      tab.terminal?.writeln(`${translate('terminal.connected_message')}\r\n`);
      sendResize(tab);
      refreshCwd(tab);
    };

    ws.onmessage = (event) => {
      if (!tab.terminal || !(event.data instanceof ArrayBuffer)) return;
      // New output means the cursor is about to move — clear any in-flight
      // ghost/popup first so stale suggestions never corrupt the display.
      clearGhost(tab);
      hidePopup();
      tab.terminal.write(textDecoder.decode(event.data));
    };

    ws.onerror = () => {
      tab.terminal?.writeln(`\r\n\x1b[31m${translate('terminal.connection_error')}\x1b[0m\r\n`);
    };

    ws.onclose = () => {
      if (destroyed) return;
      tab.connected = false;
      tabs = tabs;
      // A tab that was explicitly closed has already had cleanupTab() run,
      // which nulls tab.ws — bail out so we don't schedule a reconnect for
      // a shell we asked the server to kill.
      if (tab.ws !== ws) return;
      tab.terminal?.writeln(`\r\n\x1b[33m${translate('terminal.connection_closed')}\x1b[0m\r\n`);
      tab.reconnectTimer = setTimeout(() => {
        if (destroyed || tab.ws !== ws) return;
        tab.terminal?.writeln(`${translate('terminal.reconnecting')}\r\n`);
        connectTab(tab);
      }, 3000);
    };
  }

  function sendInput(tab: Tab | null, data: string) {
    if (tab?.ws && tab.ws.readyState === WebSocket.OPEN) {
      tab.ws.send(textEncoder.encode(data));
    }
  }

  function sendResize(tab: Tab | null) {
    if (tab?.ws && tab.ws.readyState === WebSocket.OPEN && tab.terminal) {
      tab.ws.send(JSON.stringify({ cols: tab.terminal.cols, rows: tab.terminal.rows }));
    }
  }

  function cleanupTab(tab: Tab) {
    if (tab.reconnectTimer) {
      clearTimeout(tab.reconnectTimer);
      tab.reconnectTimer = null;
    }
    if (tab.compTimer) {
      clearTimeout(tab.compTimer);
      tab.compTimer = null;
    }
    if (tab.cwdTimer) {
      clearTimeout(tab.cwdTimer);
      tab.cwdTimer = null;
    }
    if (tab.resizeObserver) {
      tab.resizeObserver.disconnect();
      tab.resizeObserver = null;
    }
    if (tab.ws) {
      const ws = tab.ws;
      tab.ws = null;
      ws.close();
    }
    if (tab.terminal) {
      tab.terminal.dispose();
      tab.terminal = null;
    }
  }

  // -------------------------------------------------------------------------
  // Completion: ghost text, Tab handling, candidate popup
  // -------------------------------------------------------------------------

  /** Entry point for every keystroke — the previous raw `sendInput` path. */
  function handleTerminalInput(tab: Tab, data: string) {
    clearGhost(tab);
    hidePopup();
    updateTrackedLine(tab, data);

    if (data === '\t') {
      void handleTab(tab);
      return; // Tab never reaches the shell in our handled cases
    }

    sendInput(tab, data);
    scheduleCompletion(tab);
  }

  function updateTrackedLine(tab: Tab, data: string) {
    if (data === '\t') return;
    if (data === '\x7f' || data === '\x08') {
      tab.inputLine = Array.from(tab.inputLine).slice(0, -1).join('');
      return;
    }
    // Escape sequences (arrows, home/end, …) and other control keys mean we
    // can no longer reliably reconstruct the line — stop tracking.
    if (data.includes('\x1b') || (data.length === 1 && data.charCodeAt(0) < 32)) {
      tab.inputLine = '';
      return;
    }
    tab.inputLine += data;
  }

  /** Debounced recompute of the current token's completion candidates. */
  function scheduleCompletion(tab: Tab) {
    if (tab.compTimer) clearTimeout(tab.compTimer);
    tab.compTimer = setTimeout(() => computeCompletions(tab), 180);
  }

  async function computeCompletions(tab: Tab) {
    if (tab.id !== activeTabId || !tab.started) return;
    const line = tab.inputLine;
    if (!line.trim()) {
      tab.candidates = [];
      tab.ghostSuffix = '';
      return;
    }
    const { token, tokenStart } = parseCurrentToken(line);
    if (!token) return;

    let candidates: CompletionCandidate[];
    if (tokenStart === 0) {
      candidates = COMMANDS.filter((c) => c.startsWith(token)).map((c) => ({ name: c, isDir: false }));
    } else {
      candidates = await pathCompletions(tab, token);
    }
    if (tab.inputLine !== line) return; // the line moved while we computed
    tab.candidates = candidates;

    // Exactly one candidate → inline ghost (the headline UX). Directories get
    // a trailing `/` so accepting a folder feels natural.
    if (candidates.length === 1) {
      const c = candidates[0];
      const suffix = c.name.slice(token.length);
      if (suffix) {
        tab.ghostSuffix = suffix;
        renderGhost(tab, suffix);
      } else {
        tab.ghostSuffix = '';
      }
    } else {
      tab.ghostSuffix = '';
    }
  }

  async function pathCompletions(tab: Tab, token: string): Promise<CompletionCandidate[]> {
    const cwd = await ensureCwd(tab);
    if (!cwd) return [];

    let dir: string;
    let prefix: string;
    if (token.startsWith('/')) {
      const slash = token.lastIndexOf('/');
      dir = token.slice(0, slash + 1) || '/';
      prefix = token.slice(slash + 1);
    } else {
      const slash = token.lastIndexOf('/');
      if (slash >= 0) {
        dir = `${cwd.replace(/\/$/, '')}/${token.slice(0, slash + 1)}`;
        prefix = token.slice(slash + 1);
      } else {
        dir = cwd;
        prefix = token;
      }
    }

    const r = await api.listTerminalDir(tab.id, dir);
    if (!r.success || !r.data?.entries) return [];
    const hideDotfiles = !prefix.startsWith('.');
    return r.data.entries
      .filter((e) => (hideDotfiles && e.name.startsWith('.') ? false : e.name.startsWith(prefix)))
      .map((e) => ({ name: e.name + (e.is_dir ? '/' : ''), isDir: e.is_dir }));
  }

  /** cwd is cached per tab and refreshed at most once per 2 s. */
  async function ensureCwd(tab: Tab): Promise<string> {
    if (tab.cwd) return tab.cwd;
    await refreshCwd(tab);
    return tab.cwd;
  }

  async function refreshCwd(tab: Tab) {
    if (tab.cwdTimer) return; // a refresh is already in flight
    tab.cwdTimer = setTimeout(() => {
      tab.cwdTimer = null;
    }, 2000);
    const r = await api.getTerminalCwd(tab.id);
    if (r.success && r.data?.cwd) {
      tab.cwd = r.data.cwd;
    }
  }

  /** Renders the suggestion as a dim italic suffix right after the cursor. */
  function renderGhost(tab: Tab, suffix: string) {
    if (!tab.terminal) return;
    const term = tab.terminal;
    const row = term.buffer.active.cursorY;
    const col = term.buffer.active.cursorX;
    term.write(`\x1b[2m\x1b[3m${suffix}\x1b[23m\x1b[22m`);
    tab.ghost = { row, col, text: suffix };
  }

  /** Overwrites the ghost region with spaces and parks the cursor back where
   *  it started. Safe because we always clear *before* the cursor can move
   *  (the next keystroke or the next server frame). */
  function clearGhost(tab: Tab) {
    if (!tab.terminal || !tab.ghost) return;
    const len = Array.from(tab.ghost.text).length;
    if (len > 0) {
      tab.terminal.write(`\x1b[${len}D`);
      tab.terminal.write(' '.repeat(len));
      tab.terminal.write(`\x1b[${len}D`);
    }
    tab.ghost = null;
  }

  async function handleTab(tab: Tab) {
    // If the debounced computation is still pending, resolve it *now* so a
    // Tab pressed right after typing acts on fresh candidates rather than
    // the empty default.
    if (tab.compTimer) {
      clearTimeout(tab.compTimer);
      tab.compTimer = null;
      await computeCompletions(tab);
    }
    // Accept the single candidate ghost first (inline completion).
    if (tab.ghostSuffix) {
      const suffix = tab.ghostSuffix;
      tab.ghostSuffix = '';
      clearGhost(tab);
      sendInput(tab, suffix);
      tab.inputLine += suffix;
      scheduleCompletion(tab);
      return;
    }
    // Several candidates → toggle the popup; a second Tab falls through to the
    // shell's own native completion.
    if (tab.candidates.length > 1) {
      if (popupVisible) {
        hidePopup();
        sendInput(tab, '\t');
      } else {
        popupCandidates = tab.candidates.slice(0, 12);
        popupVisible = true;
      }
      return;
    }
    // No suggestion of ours → the shell handles Tab natively.
    sendInput(tab, '\t');
  }

  /** Accept a candidate from the popup (click). */
  function completeWith(name: string) {
    const tab = activeTab;
    if (!tab) return;
    clearGhost(tab);
    hidePopup();
    const { token } = parseCurrentToken(tab.inputLine);
    const suffix = name.slice(token.length);
    if (suffix) {
      sendInput(tab, suffix);
      tab.inputLine += suffix;
    }
    scheduleCompletion(tab);
  }

  function hidePopup() {
    popupVisible = false;
    popupCandidates = [];
  }

</script>
<div class="h-full w-full flex flex-col bg-[var(--hv-shell-glass)] overflow-hidden" data-testid="terminal-page">
  <WindowHeader title={$t('terminal.title')} accent="var(--hv-app-terminal-grad)">
    {#snippet icon()}
      <TerminalIcon stroke={1.8} />
    {/snippet}
  </WindowHeader>

  <!-- Tab strip — window content, directly under WindowHeader and above the
       info bar. It is deliberately NOT part of WindowHeader: that bar carries
       the app title (and icon) only, and a tab strip there competes with the
       window controls pinned to its right edge.
       `min-h` not `h`: under `@media (any-pointer: coarse)` base.css forces a
       44px floor on every `button`, so the chips can grow past an exact 44px
       bar — the bar absorbs that instead of clipping them.
       `overflow-y-hidden` is required next to `overflow-x-auto`: a lone
       `overflow-x: auto` computes `overflow-y` to `auto` as well, and the 1px
       a `min-h-11` border-box leaves over its content box (44px minus the 1px
       bottom border) is all a vertical scrollbar needs. The pre-migration rule
       hid both bars with `scrollbar-width: none`, which has no Tailwind
       utility and was lost — so remove this axis at the source rather than
       hiding bars: the horizontal bar is still there when tabs overflow. -->
  <div data-testid="terminal-tabs" class="flex-none flex items-center gap-1 min-h-11 px-3 overflow-x-auto overflow-y-hidden bg-[var(--hv-shell-glass)] border-b border-[var(--hv-shell-border)]">
    {#each tabs as tab (tab.id)}
      <button
        data-testid="terminal-tab"
        class:hv-tab-active={tab.id === activeTabId}
        class={cn('inline-flex items-center gap-1.5 h-8 px-2.5 rounded-lg border-none bg-transparent text-c-base font-medium cursor-pointer whitespace-nowrap', tab.id === activeTabId ? 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)]' : 'text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]')}
        onclick={() => activateTab(tab.id)}
      >
        <span data-testid="terminal-tab-dot" class:connected={tab.connected} class={cn('w-1.5 h-1.5 rounded-full flex-none', tab.connected ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
        <span data-testid="terminal-tab-title" class="truncate max-w-[120px]">{tab.title}</span>
        {#if tabs.length > 1}
          <span data-testid="terminal-tab-close" class="grid place-items-center w-7 h-7 hv-ic rounded border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-error)]" role="button" tabindex="0" aria-label={$t('terminal.close_tab')} onclick={(e) => { e.stopPropagation(); closeTab(tab.id); }} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); closeTab(tab.id); } }}>
            <IconX size={14} stroke={2.4} />
          </span>
        {/if}
      </button>
    {/each}
    {#if tabs.length < MAX_TABS}
      <button data-testid="terminal-tab-add" class="grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)] hv-ic" onclick={addTab} aria-label={$t('terminal.new_tab')} title={$t('terminal.new_tab')}>
        <IconPlus size={20} stroke={2.2} />
      </button>
    {/if}
  </div>

  <div data-testid="terminal-infobar" class="flex items-center gap-2 px-3 h-8 bg-[var(--hv-shell-border-soft)] border-b border-[var(--hv-shell-border)] text-c-xs text-[var(--hv-shell-text-2)]">
    <span data-testid="terminal-info-user" class="inline-flex items-center gap-1 font-semibold text-[var(--hv-shell-text-strong)]">
      <IconDeviceDesktop size={17} stroke={1.8} />
      {username}@{hostname}
    </span>
    <span class="hv-info-sep w-px h-3 bg-[var(--hv-shell-border)]"></span>
    <span data-testid="terminal-info-cwd" title={activeTab?.cwd || ''}>{activeTab?.cwd || '~'}</span>
    <span class="flex-1"></span>
    <span data-testid="terminal-info-badge" class:connected={activeTab?.connected} class={cn('inline-flex items-center px-2 py-0.5 rounded-full text-micro font-bold uppercase', activeTab?.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
      {$t('terminal.ssh')} · local
    </span>
    <span data-testid="terminal-info-shell">{$t('terminal.shell_version')}</span>
  </div>

  <!-- The body paints the SAME background as the xterm theme above it
       (`theme.background` = #FFFFFF, i.e. --hv-shell-glass). FitAddon sizes the
       terminal to `floor(height / cellHeight)` rows, so a sub-row remainder of
       up to one cell height is left at the bottom; with a contrasting
       background here that remainder painted as a black band under the last
       line. Never darken this container without darkening the theme too. -->
  <div data-testid="terminal-body" class="relative flex-1 min-h-0 bg-[var(--hv-shell-glass)]">
    {#each tabs as tab (tab.id)}
      <div class="hv-terminal-content h-full w-full" bind:this={tab.container} hidden={tab.id !== activeTabId}></div>
    {/each}
    {#if popupVisible && popupCandidates.length > 0}
      <div
        data-testid="terminal-completion-popup"
        class="absolute bottom-2 left-1/2 -translate-x-1/2 z-10 w-[min(560px,calc(100%-2rem))] max-h-48 overflow-y-auto rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] shadow-[var(--hv-shell-shadow-card)] p-1.5"
      >
        <p class="m-0 px-2 pb-1 text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
          {popupCandidates.length}{$t('terminal.completions_suffix')}
        </p>
        <div class="flex flex-col">
          {#each popupCandidates as cand (cand.name)}
            <button
              data-testid="terminal-completion-item"
              class="flex items-center gap-2 px-2.5 py-1.5 rounded-lg border-none bg-transparent text-left text-c-sm font-mono text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]"
              onclick={() => completeWith(cand.name)}
            >
              <span class="truncate">{cand.name}</span>
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>
</div>