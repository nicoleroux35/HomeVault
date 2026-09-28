<script lang="ts">
  /**
   * Virtual Machines — built so a person with no Linux knowledge can create a
   * machine, log into it and throw it away. One list, one wizard, one detail
   * drawer, one console. Everything else the backend decides.
   *
   * Wire contract mirrors src/handlers/vm.rs exactly.
   */
  import {
    IconServer, IconBox, IconPlus, IconX, IconPlayerPlay, IconPlayerStop,
    IconRefresh, IconTrash, IconTerminal2, IconCamera, IconRotateClockwise,
    IconChevronRight, IconCopy, IconCheck, IconAlertTriangle, IconLoader2,
    IconCpu, IconDatabase, IconDeviceSdCard, IconWorld, IconArrowBackUp,
    IconSettings, IconEye, IconEyeOff, IconChevronDown, IconDeviceDesktop, IconPackages,
  } from '@tabler/icons-svelte';
  import { t, translate } from '$lib/stores/i18n';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import VmConsole from '$lib/components/vm/VmConsole.svelte';
  import VmScreen from '$lib/components/vm/VmScreen.svelte';
  import { Button, Modal, Alert } from '$lib/components/theme';
  import { PageLayout, actionBtn, actionIconBtn, textInput } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import PathPickerField from '$lib/components/theme/PathPickerField.svelte';

  // ── Types (match src/handlers/vm.rs) ──────────────────────────────────────
  interface Envelope<T = unknown> { success: boolean; data?: T; message?: string }
  type VmStatus = 'running' | 'stopped' | 'starting' | 'stopping' | 'error' | 'unknown';
  interface VmInfo {
    name: string;
    kind: 'vm' | 'container';
    status: VmStatus;
    running: boolean;
    cpu_cores: number | null;
    memory_mb: number | null;
    disk_gb: number | null;
    ipv4: string | null;
    os: string | null;
    created_at: string;
    snapshot_count: number;
    has_screen: boolean;
  }
  interface Snapshot { name: string; created_at: string; stateful: boolean }
  interface VmDetail extends VmInfo {
    login_user: string | null;
    memory_used_mb: number | null;
    disk_used_gb: number | null;
    snapshots: Snapshot[];
  }
  interface OsOption {
    id: string; label: string; kinds: string[]; alias: string;
    blurb: string; recommended: boolean;
  }

  // ── Local authed fetch (same pattern as Docker.svelte) ────────────────────
  async function call<T = unknown>(method: string, endpoint: string, body?: unknown): Promise<Envelope<T>> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    const st = localStorage.getItem('session-token');
    const ct = localStorage.getItem('csrf-token');
    if (st) headers['X-Session-Token'] = st;
    if (ct) headers['X-CSRF-Token'] = ct;
    try {
      const res = await fetch('/api' + endpoint, {
        method, headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      return await res.json();
    } catch (e) {
      return { success: false, message: e instanceof Error ? e.message : 'Network error' };
    }
  }

  // ── State ────────────────────────────────────────────────────────────────
  let ready = $state(false);
  let incusOk = $state<boolean | null>(null);
  let machines = $state<VmInfo[]>([]);
  let osOptions = $state<OsOption[]>([]);
  let busyName = $state<string | null>(null);       // a machine mid-action
  let creating = $state<string[]>([]);              // names being provisioned
  let banner = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let openMenu = $state<string | null>(null);

  let consoleVm = $state<string | null>(null);
  let detailVm = $state<string | null>(null);
  let detail = $state<VmDetail | null>(null);
  let detailLoading = $state(false);

  // Wizard
  let wizardOpen = $state(false);
  let wMode = $state<'catalogue' | 'iso'>('catalogue');
  let wOs = $state<string>('');
  let wKind = $state<'vm' | 'container'>('vm');
  let wName = $state('');
  let wNameTouched = $state(false);
  let wSize = $state<'small' | 'medium' | 'large'>('medium');
  let wUser = $state('admin');
  let wPass = $state('');
  let wShowPass = $state(false);
  let wAdvanced = $state(false);
  let wCpu = $state(2);
  let wMem = $state(2048);
  let wDisk = $state(25);
  let wBusy = $state(false);
  // ISO mode
  let wIsoOs = $state<'linux' | 'windows'>('linux');
  let wIsoPath = $state('');
  let wDriversIso = $state('');
  let wSecureBoot = $state(true);
  interface ManagedIso { name: string; path: string; size: number; virtio: boolean }
  let managedIsos = $state<ManagedIso[]>([]);
  let isoUrl = $state('');
  let isoUrlOpen = $state(false);
  let isoBusy = $state(false);
  let storeIsos = $derived(managedIsos.filter(i => !i.virtio));
  let virtioIso = $derived(managedIsos.find(i => i.virtio) ?? null);

  let screenVm = $state<string | null>(null);

  // Snapshot dialog
  let snapOpen = $state(false);
  let snapName = $state('');
  let snapBusy = $state(false);

  // Delete confirm
  let confirmDelete = $state<string | null>(null);
  let confirmRestore = $state<Snapshot | null>(null);

  let copied = $state(false);

  const SIZE_SPECS: Record<string, { cpu: number; mem: number; disk: number }> = {
    small: { cpu: 1, mem: 1024, disk: 10 },
    medium: { cpu: 2, mem: 2048, disk: 25 },
    large: { cpu: 4, mem: 8192, disk: 60 },
  };

  // ── Derived ──────────────────────────────────────────────────────────────
  let runningCount = $derived(machines.filter(m => m.running).length);
  let selectedOs = $derived(osOptions.find(o => o.id === wOs) ?? null);
  let osForKind = $derived(
    osOptions.filter(o => o.kinds.includes(wKind === 'vm' ? 'vm' : 'container'))
  );
  let nameError = $derived(validateName(wName));
  let passError = $derived(wPass.length > 0 && wPass.length < 6 ? translate('vm.w_pass_short') : '');
  let isoError = $derived(
    wIsoPath && !/\.iso$/i.test(wIsoPath.trim()) ? translate('vm.w_iso_ext') : ''
  );
  let canCreate = $derived(
    !!wName && !nameError && !wBusy &&
    (wMode === 'catalogue'
      ? (!!wOs && wPass.length >= 6 && !!wUser)
      : (!!wIsoPath.trim() && !isoError))
  );

  function validateName(n: string): string {
    if (!n) return '';
    if (!/^[a-z0-9-]+$/.test(n)) return translate('vm.w_name_chars');
    if (n.length > 63) return translate('vm.w_name_long');
    if (n.startsWith('-') || n.endsWith('-') || n.includes('--')) return translate('vm.w_name_dash');
    if (machines.some(m => m.name === n) || creating.includes(n)) return translate('vm.w_name_taken');
    return '';
  }

  // ── Formatting ───────────────────────────────────────────────────────────
  function fmtMem(mb: number | null): string {
    if (mb == null) return '—';
    return mb >= 1024 ? `${(mb / 1024).toFixed(mb % 1024 === 0 ? 0 : 1)} ${translate('vm.unit_gb')}` : `${mb} ${translate('vm.unit_mb')}`;
  }
  function fmtDisk(gb: number | null): string {
    if (gb == null) return '—';
    return gb >= 1024 ? `${(gb / 1024).toFixed(1)} ${translate('vm.unit_tb')}` : `${gb} ${translate('vm.unit_gb')}`;
  }
  function statusLabel(s: VmStatus): string { return translate(`vm.status_${s}`); }

  // ── Load ─────────────────────────────────────────────────────────────────
  async function load() {
    const [av, os, list] = await Promise.all([
      call<{ available: boolean }>('GET', '/vm/available'),
      call<OsOption[]>('GET', '/vm/os-options'),
      call<VmInfo[]>('GET', '/vm/list'),
    ]);
    incusOk = !!(av.success && av.data?.available);
    ready = true;
    if (os.success && os.data) osOptions = os.data;
    if (!incusOk) return;
    if (list.success && Array.isArray(list.data)) {
      machines = list.data;
      // Drop "creating" entries that have now materialised.
      creating = creating.filter(n => !machines.some(m => m.name === n));
    } else if (list.message) {
      setBanner('error', list.message);
    }
  }

  onAppVisible(load, { pollMs: 8000 });

  // ── Master-detail split ──────────────────────────────────────────────────
  // `narrow` measures the split container itself (the window floats, so a
  // viewport media query would lie). Below 720px only one pane shows — the
  // list, or the detail drawer when a machine is selected.
  let splitEl = $state<HTMLDivElement | null>(null);
  let narrow = $state(false);
  let splitRO: ResizeObserver | null = null;
  $effect(() => {
    if (!splitEl) return;
    splitRO?.disconnect();
    splitRO = new ResizeObserver((entries) => {
      narrow = (entries[0]?.contentRect.width ?? 0) < 720;
    });
    splitRO.observe(splitEl);
    return () => {
      splitRO?.disconnect();
      splitRO = null;
    };
  });

  function setBanner(kind: 'ok' | 'error', text: string) {
    banner = { kind, text };
    if (kind === 'ok') setTimeout(() => { if (banner?.text === text) banner = null; }, 4500);
  }

  // ── Machine actions ──────────────────────────────────────────────────────
  async function act(name: string, action: 'start' | 'stop' | 'restart') {
    openMenu = null;
    busyName = name;
    const res = await call('PUT', `/vm/${encodeURIComponent(name)}/${action}`);
    busyName = null;
    if (res.success) {
      setBanner('ok', typeof res.data === 'string' ? res.data : translate('vm.action_ok'));
    } else {
      setBanner('error', res.message || translate('vm.action_failed'));
    }
    await load();
  }

  async function doDelete(name: string) {
    confirmDelete = null;
    busyName = name;
    const res = await call('DELETE', `/vm/${encodeURIComponent(name)}`);
    busyName = null;
    if (res.success) {
      setBanner('ok', translate('vm.deleted', { name }));
      if (detailVm === name) closeDetail();
    } else {
      setBanner('error', res.message || translate('vm.delete_failed'));
    }
    await load();
  }

  // ── Detail drawer ────────────────────────────────────────────────────────
  async function openDetail(name: string) {
    detailVm = name;
    detail = null;
    detailLoading = true;
    openMenu = null;
    const res = await call<VmDetail>('GET', `/vm/${encodeURIComponent(name)}`);
    detailLoading = false;
    if (res.success && res.data) detail = res.data;
    else { setBanner('error', res.message || translate('vm.load_failed')); detailVm = null; }
  }
  function closeDetail() { detailVm = null; detail = null; }
  async function refreshDetail() {
    if (!detailVm) return;
    const res = await call<VmDetail>('GET', `/vm/${encodeURIComponent(detailVm)}`);
    if (res.success && res.data) detail = res.data;
  }

  async function saveResize(cpu: number, mem: number) {
    if (!detail) return;
    const res = await call('PUT', `/vm/${encodeURIComponent(detail.name)}/config`, {
      cpu_cores: cpu, memory_mb: mem,
    });
    if (res.success) setBanner('ok', typeof res.data === 'string' ? res.data : translate('vm.action_ok'));
    else setBanner('error', res.message || translate('vm.action_failed'));
    await Promise.all([load(), refreshDetail()]);
  }

  // ── Snapshots ────────────────────────────────────────────────────────────
  function openSnap() { snapName = ''; snapOpen = true; }
  async function createSnap() {
    if (!detail || !snapName.trim()) return;
    snapBusy = true;
    const res = await call('POST', `/vm/${encodeURIComponent(detail.name)}/snapshots`, {
      name: snapName.trim(), stateful: false,
    });
    snapBusy = false;
    if (res.success) { setBanner('ok', translate('vm.snap_saved', { name: snapName.trim() })); snapOpen = false; }
    else setBanner('error', res.message || translate('vm.action_failed'));
    await Promise.all([load(), refreshDetail()]);
  }
  async function restoreSnap(s: Snapshot) {
    confirmRestore = null;
    if (!detail) return;
    busyName = detail.name;
    const res = await call('POST', `/vm/${encodeURIComponent(detail.name)}/snapshots/${encodeURIComponent(s.name)}/restore`);
    busyName = null;
    if (res.success) setBanner('ok', typeof res.data === 'string' ? res.data : translate('vm.action_ok'));
    else setBanner('error', res.message || translate('vm.action_failed'));
    await Promise.all([load(), refreshDetail()]);
  }
  async function deleteSnap(s: Snapshot) {
    if (!detail) return;
    const res = await call('DELETE', `/vm/${encodeURIComponent(detail.name)}/snapshots/${encodeURIComponent(s.name)}`);
    if (res.success) setBanner('ok', translate('vm.snap_deleted', { name: s.name }));
    else setBanner('error', res.message || translate('vm.action_failed'));
    await Promise.all([load(), refreshDetail()]);
  }

  // ── Wizard ───────────────────────────────────────────────────────────────
  function openWizard() {
    wizardOpen = true; wMode = 'catalogue';
    wOs = osOptions.find(o => o.recommended)?.id ?? osOptions[0]?.id ?? '';
    wKind = 'vm'; wNameTouched = false; wSize = 'medium';
    wUser = 'admin'; wPass = ''; wShowPass = false;
    wAdvanced = false; wCpu = 2; wMem = 2048; wDisk = 25;
    wIsoOs = 'linux'; wIsoPath = ''; wDriversIso = ''; wSecureBoot = true;
    isoUrl = ''; isoUrlOpen = false;
    suggestName(wOs);
  }
  function setMode(m: 'catalogue' | 'iso') {
    wMode = m;
    if (m === 'iso') loadIsos();
    if (!wNameTouched) suggestName(m === 'iso' ? (wIsoOs === 'windows' ? 'windows' : 'linux') : wOs);
  }

  // ── Managed ISO store ────────────────────────────────────────────────────
  async function loadIsos() {
    const r = await call<ManagedIso[]>('GET', '/vm/isos');
    if (r.success && Array.isArray(r.data)) managedIsos = r.data;
  }
  async function downloadIso() {
    const url = isoUrl.trim();
    if (!/^https:\/\/.+\.iso(\?.*)?$/i.test(url)) return;
    isoBusy = true;
    const r = await call<{ name: string }>('POST', '/vm/isos/download', { url });
    isoBusy = false;
    if (r.success) {
      setBanner('ok', translate('vm.iso_download_started', { name: r.data?.name ?? '' }));
      isoUrl = ''; isoUrlOpen = false;
      pollIsos();
    } else setBanner('error', r.message || translate('vm.iso_download_failed'));
  }
  async function downloadVirtio() {
    isoBusy = true;
    const r = await call<{ name: string }>('POST', '/vm/isos/virtio');
    isoBusy = false;
    if (r.success) { setBanner('ok', translate('vm.iso_download_started', { name: 'virtio-win.iso' })); pollIsos(); }
    else setBanner('error', r.message || translate('vm.iso_download_failed'));
  }
  async function deleteIso(name: string) {
    const r = await call('DELETE', `/vm/isos/${encodeURIComponent(name)}`);
    if (r.success) {
      setBanner('ok', translate('vm.iso_deleted', { name }));
      if (managedIsos.find(i => i.name === name)?.path === wIsoPath) wIsoPath = '';
      await loadIsos();
    } else setBanner('error', r.message || translate('vm.action_failed'));
  }
  function pollIsos() {
    let n = 0;
    const iv = setInterval(async () => {
      await loadIsos();
      if (++n > 240 || !wizardOpen) clearInterval(iv);
    }, 5000);
  }
  function fmtBytes(b: number): string {
    if (b < 1073741824) return `${(b / 1048576).toFixed(0)} ${translate('vm.unit_mb')}`;
    return `${(b / 1073741824).toFixed(1)} ${translate('vm.unit_gb')}`;
  }
  function setIsoOs(o: 'linux' | 'windows') {
    wIsoOs = o;
    wSecureBoot = o === 'windows';
    if (!wNameTouched) suggestName(o);
  }
  function pickOs(id: string) {
    wOs = id;
    const o = osOptions.find(x => x.id === id);
    if (o && !o.kinds.includes(wKind === 'vm' ? 'vm' : 'container')) {
      wKind = o.kinds.includes('vm') ? 'vm' : 'container';
    }
    if (!wNameTouched) suggestName(id);
  }
  function suggestName(osId: string) {
    const base = (osId || 'machine').replace(/[^a-z0-9]+/g, '-').replace(/-+/g, '-').replace(/(^-|-$)/g, '');
    let n = base, i = 1;
    while (machines.some(m => m.name === n) || creating.includes(n)) { n = `${base}-${++i}`; }
    wName = n;
  }
  function applySize(s: 'small' | 'medium' | 'large') {
    wSize = s;
    wCpu = SIZE_SPECS[s].cpu; wMem = SIZE_SPECS[s].mem; wDisk = SIZE_SPECS[s].disk;
  }

  async function submitWizard() {
    if (!canCreate) return;
    wBusy = true;
    const payload: Record<string, unknown> = { name: wName, size: wSize };
    if (wMode === 'catalogue') {
      payload.kind = wKind;
      payload.os = wOs;
      payload.username = wUser.trim();
      payload.password = wPass;
    } else {
      payload.iso_path = wIsoPath.trim();
      payload.iso_os = wIsoOs;
      payload.secure_boot = wSecureBoot;
      if (wIsoOs === 'windows' && wDriversIso.trim()) payload.drivers_iso_path = wDriversIso.trim();
    }
    if (wAdvanced) { payload.cpu_cores = wCpu; payload.memory_mb = wMem; payload.disk_gb = wDisk; }
    const res = await call<{ name: string }>('POST', '/vm/create', payload);
    wBusy = false;
    if (res.success) {
      creating = [...creating, wName];
      setBanner('ok', translate('vm.create_started', { name: wName }));
      wizardOpen = false;
      await load();
    } else {
      setBanner('error', res.message || translate('vm.create_failed'));
    }
  }

  async function copyConnect() {
    if (!detail?.ipv4 || !detail.login_user) return;
    try {
      await navigator.clipboard.writeText(`ssh ${detail.login_user}@${detail.ipv4}`);
      copied = true; setTimeout(() => copied = false, 1600);
    } catch { /* ignore */ }
  }

  // The desktop window frame carries a `transform`, so a `position: fixed`
  // child is positioned/clipped against the window, not the viewport. Portal
  // every full-screen overlay to <body>.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  function onKey(e: KeyboardEvent) {
    if (e.key !== 'Escape') return;
    if (screenVm) screenVm = null;
    else if (consoleVm) consoleVm = null;
    else if (confirmRestore) confirmRestore = null;
    else if (confirmDelete) confirmDelete = null;
    else if (snapOpen) snapOpen = false;
    else if (wizardOpen) wizardOpen = false;
    else if (detailVm) closeDetail();
    else openMenu = null;
  }
  // Shared control shapes (actionBtn / actionIconBtn / textInput) come from
  // components/app controlClasses.ts. The dead `vm-btn*` hook prefixes were
  // removed with them — no CSS rule ever referenced them.
  const vmPill = 'vm-pill inline-flex items-center px-2 py-0.5 rounded-full text-c-xs font-bold uppercase';
  const vmCard = 'vm-card flex items-center gap-3 px-4 py-3 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] cursor-pointer hover:border-[var(--hv-shell-border-strong)]';
  const vmEmpty = 'vm-empty flex flex-col items-center gap-3 py-16 text-center text-[var(--hv-shell-text)]';
  const vmField = 'vm-field flex flex-col gap-1.5';
  const vmFieldLabel = 'vm-field-label text-xs font-semibold text-[var(--hv-shell-text-strong)]';
  const vmFieldErr = 'vm-field-err text-xs text-[var(--hv-error-text)]';
  const vmFieldHint = 'vm-field-hint text-c-xs text-[var(--hv-shell-text-2)]';
  const vmSel = 'vm-sel';
  const vmCardName = 'vm-card-name text-sm font-bold text-[var(--hv-shell-text-strong)]';
  const vmKindToggleBtn = 'flex-1 h-10 rounded-lg border border-[var(--hv-shell-border)] bg-transparent text-c-base font-semibold text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]';
</script>

<svelte:window onkeydown={onKey} onclick={() => openMenu = null} />

<PageLayout
  title={$t('vm.title')}
  data-testid="vm-page"
>
  {#snippet icon()}<IconServer stroke={1.7} />{/snippet}

  <!-- Page-level primary action: a WindowToolbar row under the header, never
       inside it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="primary" data-testid="vm-new" onclick={openWizard} disabled={incusOk === false || !ready}>
      <IconPlus size={16} stroke={2} />
      <span>{$t('vm.new')}</span>
    </Button>
  {/snippet}

  {#if banner}
    <div class="mb-3"><Alert variant={banner.kind === 'error' ? 'error' : 'success'} dismissible ondismiss={() => banner = null}>{banner.text}</Alert></div>
  {/if}

  <div data-testid="vm-split" bind:this={splitEl} class="flex flex-1 min-h-0">
    {#if !(narrow && detailVm)}
      <div data-testid="vm-list-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto">
          {#if !ready}
    <div class="flex items-center justify-center py-20"><IconLoader2 class="vm-spin" size={30} stroke={1.8} /></div>
  {:else if incusOk === false}
    <div class={vmEmpty}>
      <IconServer size={46} stroke={1.1} class="text-[var(--hv-shell-border-strong)]" />
      <h2 class="m-0 text-lg font-semibold text-[var(--hv-shell-text-2)]">{$t('vm.unavailable')}</h2>
      <p class="m-0 text-sm">{$t('vm.unavailable_desc')}</p>
    </div>
  {:else if machines.length === 0 && creating.length === 0}
    <div data-testid="vm-empty" class={vmEmpty}>
      <IconServer size={46} stroke={1.1} class="text-[var(--hv-shell-border-strong)]" />
      <h2 class="m-0 text-lg font-semibold text-[var(--hv-shell-text-2)]">{$t('vm.none_title')}</h2>
      <p class="m-0 text-sm">{$t('vm.none_desc')}</p>
      <Button variant="primary" data-testid="vm-cta" class="vm-cta" onclick={openWizard}><IconPlus size={17} stroke={2} /> {$t('vm.none_cta')}</Button>
    </div>
  {:else}
    <div data-testid="vm-list" class="flex flex-col gap-2.5">
      {#each creating as name (name)}
        <div data-testid="vm-card-creating" class={cn(vmCard, 'vm-card-creating cursor-default')}>
          <span class="grid place-items-center w-10 h-10 rounded-xl flex-none bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]"><IconLoader2 class="vm-spin" size={20} stroke={2} /></span>
          <div class="flex-1 min-w-0">
            <div data-testid="vm-card-name" class={vmCardName}>{name}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('vm.creating_sub')}</div>
          </div>
          <span data-testid="vm-pill" data-status="creating" class={cn(vmPill, 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}>{$t('vm.status_starting')}</span>
        </div>
      {/each}

      {#each machines as m (m.name)}
        <div data-testid="vm-card" class={vmCard} onclick={() => openDetail(m.name)} role="button" tabindex="0" onkeydown={(e) => activateOnEnterOrSpace(e, () => openDetail(m.name))}>
          <span class={cn('grid place-items-center w-10 h-10 rounded-xl flex-none', m.running ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : m.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
            {#if m.kind === 'container'}<IconBox size={20} stroke={1.9} />{:else}<IconServer size={20} stroke={1.9} />{/if}
          </span>
          <div class="flex-1 min-w-0">
            <div data-testid="vm-card-name" class={vmCardName}>
              {m.name}
              <span class="ml-1 text-c-xs font-semibold text-[var(--hv-shell-text-2)]">{m.kind === 'vm' ? $t('vm.kind_vm') : $t('vm.kind_container')}</span>
            </div>
            <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{m.os ?? $t('vm.os_unknown')}</div>
            <div data-testid="vm-specs" class="vm-specs flex flex-wrap gap-x-3 gap-y-0.5 mt-1 text-c-xs text-[var(--hv-shell-text-2)]">
              <span class="inline-flex items-center gap-1"><IconCpu size={13} stroke={1.8} /> {m.cpu_cores ?? '—'} {$t('vm.vcpu')}</span>
              <span class="inline-flex items-center gap-1"><IconDatabase size={13} stroke={1.8} /> {fmtMem(m.memory_mb)}</span>
              <span class="inline-flex items-center gap-1"><IconDeviceSdCard size={13} stroke={1.8} /> {fmtDisk(m.disk_gb)}</span>
              {#if m.running && m.ipv4}<span class="inline-flex items-center gap-1"><IconWorld size={13} stroke={1.8} /> {m.ipv4}</span>{/if}
              {#if m.snapshot_count > 0}<span class="inline-flex items-center gap-1"><IconCamera size={13} stroke={1.8} /> {m.snapshot_count}</span>{/if}
            </div>
          </div>
          <span data-testid="vm-pill" data-status={m.status} class={cn(vmPill, m.status === 'running' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : m.status === 'starting' || m.status === 'stopping' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : m.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{statusLabel(m.status)}</span>
          <div data-testid="vm-card-act" class="flex items-center gap-1.5 flex-none" onclick={(e) => e.stopPropagation()} role="none">
            {#if busyName === m.name}
              <button class={actionBtn} disabled><IconLoader2 class="vm-spin" size={16} stroke={2} /></button>
            {:else if m.running && m.has_screen}
              <Button variant="primary" data-testid="vm-btn-primary" onclick={() => { screenVm = m.name; }}><IconDeviceDesktop size={16} stroke={2} /> {$t('vm.screen')}</Button>
            {:else if m.running}
              <Button variant="primary" data-testid="vm-btn-primary" onclick={() => { consoleVm = m.name; }}><IconTerminal2 size={16} stroke={2} /> {$t('vm.console')}</Button>
            {:else}
              <Button variant="primary" data-testid="vm-btn-primary" onclick={() => act(m.name, 'start')}><IconPlayerPlay size={16} stroke={2} /> {$t('vm.start')}</Button>
            {/if}
            <div data-testid="vm-menu-wrap" class="vm-menu-wrap relative">
              <button class={cn(actionIconBtn, 'hv-ic')} aria-label={$t('vm.more')} aria-haspopup="menu" aria-expanded={openMenu === m.name} onclick={(e) => { e.stopPropagation(); openMenu = openMenu === m.name ? null : m.name; }}><IconChevronDown size={16} stroke={2} /></button>
              {#if openMenu === m.name}
                <div class="vm-menu absolute top-full right-0 mt-1 z-[var(--hv-z-dropdown)] w-44 rounded-xl bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-1" role="menu">
                  <button role="menuitem" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openDetail(m.name)}><IconSettings size={15} stroke={1.9} /> {$t('vm.details')}</button>
                  {#if m.running && m.has_screen}
                    <button role="menuitem" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { screenVm = m.name; openMenu = null; }}><IconDeviceDesktop size={15} stroke={1.9} /> {$t('vm.screen')}</button>
                    <button role="menuitem" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { consoleVm = m.name; openMenu = null; }}><IconTerminal2 size={15} stroke={1.9} /> {$t('vm.console')}</button>
                  {/if}
                  {#if m.running}
                    <button role="menuitem" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => act(m.name, 'restart')}><IconRotateClockwise size={15} stroke={1.9} /> {$t('vm.restart')}</button>
                    <button role="menuitem" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => act(m.name, 'stop')}><IconPlayerStop size={15} stroke={1.9} /> {$t('vm.stop')}</button>
                  {/if}
                  <button role="menuitem" data-testid="vm-menu-danger" class="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]" onclick={() => { confirmDelete = m.name; openMenu = null; }}><IconTrash size={15} stroke={1.9} /> {$t('vm.delete')}</button>
                </div>
              {/if}
            </div>
          </div>
        </div>
      {/each}
    </div>
  {/if}
      </div>
    {/if}

    {#if detailVm}
        <aside data-testid="vm-drawer" class="@container flex flex-col flex-none min-w-0 border-l border-[var(--hv-shell-border)]" style="width: {narrow ? '100%' : '420px'};" aria-label={$t('vm.details')}>
    <header class="flex items-center gap-2 px-4 py-3 border-b border-[var(--hv-shell-border)]">
      <button class={cn(actionIconBtn, 'hv-ic')} onclick={closeDetail} aria-label={$t('common.close')}><IconX size={18} stroke={2} /></button>
      <span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{detailVm}</span>
    </header>
    {#if detailLoading || !detail}
      <div class="flex items-center justify-center py-16"><IconLoader2 class="vm-spin" size={26} stroke={1.8} /></div>
    {:else}
      <div class="flex-1 min-h-0 overflow-y-auto p-4">
        <div class="flex items-center gap-3 mb-3">
          <span class={cn('grid place-items-center w-12 h-12 rounded-xl flex-none', detail.running ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : detail.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
            {#if detail.kind === 'container'}<IconBox size={26} stroke={1.8} />{:else}<IconServer size={26} stroke={1.8} />{/if}
          </span>
          <div>
            <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{detail.os ?? $t('vm.os_unknown')}</div>
            <span class={cn(vmPill, detail.status === 'running' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : detail.status === 'starting' || detail.status === 'stopping' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : detail.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{statusLabel(detail.status)}</span>
          </div>
        </div>
        <div class="flex flex-wrap gap-2 mb-3">
          {#if detail.running}
            {#if detail.has_screen}
              <Button variant="primary" onclick={() => { screenVm = detail!.name; }}><IconDeviceDesktop size={16} stroke={2} /> {$t('vm.screen')}</Button>
              <Button variant="secondary" onclick={() => { consoleVm = detail!.name; }}><IconTerminal2 size={16} stroke={1.9} /> {$t('vm.console')}</Button>
            {:else}
              <Button variant="primary" onclick={() => { consoleVm = detail!.name; }}><IconTerminal2 size={16} stroke={2} /> {$t('vm.console')}</Button>
            {/if}
            <Button variant="secondary" onclick={() => act(detail!.name, 'restart')}><IconRotateClockwise size={16} stroke={1.9} /> {$t('vm.restart')}</Button>
            <Button variant="secondary" onclick={() => act(detail!.name, 'stop')}><IconPlayerStop size={16} stroke={1.9} /> {$t('vm.stop')}</Button>
          {:else}
            <Button variant="primary" onclick={() => act(detail!.name, 'start')}><IconPlayerPlay size={16} stroke={2} /> {$t('vm.start')}</Button>
          {/if}
        </div>
        {#if detail.running && detail.has_screen}
          <section class="vm-d-card p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] mb-3">
            <h3 class="m-0 mb-1 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.screen_connect_title')}</h3>
            <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('vm.screen_connect_desc')}</p>
          </section>
        {:else if detail.running && detail.ipv4 && detail.login_user}
          <section class="vm-d-card p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] mb-3">
            <h3 class="m-0 mb-1 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.connect_title')}</h3>
            <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text)]">{$t('vm.connect_desc', { user: detail.login_user, ip: detail.ipv4 })}</p>
            <div class="flex items-center gap-2">
              <code class="flex-1 px-2.5 py-1.5 rounded-lg bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] text-xs font-[family-name:var(--hv-font-mono)]">ssh {detail.login_user}@{detail.ipv4}</code>
              <button class={cn(actionIconBtn, 'h-8 w-8 hv-ic')} onclick={copyConnect} aria-label={$t('vm.copy')}>{#if copied}<IconCheck size={15} stroke={2} />{:else}<IconCopy size={15} stroke={2} />{/if}</button>
            </div>
          </section>
        {/if}
        <section class="vm-d-card p-3 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] mb-3">
          <h3 class="m-0 mb-2 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.specs_title')}</h3>
          <dl data-testid="vm-d-specs" class="m-0 grid grid-cols-2 @max-[400px]:grid-cols-1 gap-2">
            <div><dt class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('vm.vcpu')}</dt><dd class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{detail.cpu_cores ?? '—'}</dd></div>
            <div><dt class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('vm.memory')}</dt><dd class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{fmtMem(detail.memory_mb)}{#if detail.memory_used_mb != null}<span class="vm-muted text-c-xs text-[var(--hv-shell-text-2)]"> · {fmtMem(detail.memory_used_mb)} {$t('vm.used')}</span>{/if}</dd></div>
            <div><dt class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('vm.disk')}</dt><dd class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{fmtDisk(detail.disk_gb)}{#if detail.disk_used_gb != null}<span class="vm-muted text-c-xs text-[var(--hv-shell-text-2)]"> · {detail.disk_used_gb.toFixed(1)} {$t('vm.unit_gb')} {$t('vm.used')}</span>{/if}</dd></div>
            {#if detail.ipv4}<div><dt class="text-micro font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('vm.ip')}</dt><dd class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{detail.ipv4}</dd></div>{/if}
          </dl>
          <details class="vm-resize mt-2">
            <summary class="cursor-pointer text-xs font-semibold text-[var(--hv-primary)]">{$t('vm.resize_title')}</summary>
            <div class="mt-2 flex flex-col gap-2">
              <label class="flex items-center gap-2 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.vcpu')}<input type="number" class={textInput} min="1" max="64" bind:value={detail.cpu_cores} /></label>
              <label class="flex items-center gap-2 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.memory')} ({$t('vm.unit_mb')})<input type="number" class={textInput} min="256" max="131072" step="256" bind:value={detail.memory_mb} /></label>
              <Button variant="primary" onclick={() => saveResize(detail!.cpu_cores ?? 1, detail!.memory_mb ?? 512)}>{$t('common.save')}</Button>
              <p class="vm-muted m-0 text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.resize_hint')}</p>
            </div>
          </details>
        </section>
        <section class="vm-d-card p-3 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] mb-3">
          <div class="flex items-center justify-between mb-2">
            <h3 class="m-0 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.snapshots_title')}</h3>
            <Button variant="secondary" size="sm" onclick={openSnap}><IconCamera size={14} stroke={1.9} /> {$t('vm.snap_new')}</Button>
          </div>
          <p class="vm-muted m-0 mb-2 text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.snapshots_help')}</p>
          {#if detail.snapshots.length === 0}
            <p class="vm-muted m-0 text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.snap_none')}</p>
          {:else}
            <ul class="m-0 p-0 list-none flex flex-col gap-1.5">
              {#each detail.snapshots as s (s.name)}
                <li class="flex items-center justify-between gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                  <div class="min-w-0"><span class="vm-snap-name text-c-base font-medium text-[var(--hv-shell-text-strong)]">{s.name}</span>{#if s.created_at}<span class="vm-muted text-c-xs text-[var(--hv-shell-text-2)]"> · {new Date(s.created_at).toLocaleDateString()}</span>{/if}</div>
                  <div class="flex gap-1.5"><Button variant="secondary" size="sm" onclick={() => confirmRestore = s}><IconArrowBackUp size={14} stroke={1.9} /> {$t('vm.snap_restore')}</Button><button class={cn(actionIconBtn, 'h-8 w-8 hv-ic')} onclick={() => deleteSnap(s)} aria-label={$t('vm.delete')}><IconTrash size={14} stroke={1.9} /></button></div>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
        <section data-testid="vm-d-danger" class="vm-d-card p-3 rounded-xl bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/40">
          <h3 class="m-0 mb-1 text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.danger_title')}</h3>
          <p class="vm-muted m-0 mb-2 text-c-xs text-[var(--hv-shell-text)]">{$t('vm.danger_desc')}</p>
          <Button variant="danger" onclick={() => confirmDelete = detail!.name}><IconTrash size={15} stroke={1.9} /> {$t('vm.delete_machine')}</Button>
        </section>
      </div>
    {/if}
  </aside>
    {/if}
  </div>

</PageLayout>

<!-- Overlays -->
<div class="vm-overlays" use:portal>

{#if wizardOpen}
  <Modal open={wizardOpen} title={$t('vm.wizard_title')} size="lg" onclose={() => wizardOpen = false}>
    {#snippet children()}
    <div class="flex flex-col gap-3">
      <div class="vm-mode grid grid-cols-2 @max-[360px]:grid-cols-1 gap-2.5" role="group" aria-label={$t('vm.wizard_title')}>
        <button type="button" data-testid="vm-mode-btn" class:sel={wMode === 'catalogue'} class={cn('flex flex-col items-start gap-1 p-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-left cursor-pointer', wMode === 'catalogue' && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')} onclick={() => setMode('catalogue')}>
          <IconPackages size={20} stroke={1.7} />
          <span class="vm-mode-t text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('vm.mode_catalogue')}</span>
          <span class="vm-mode-h text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.mode_catalogue_hint')}</span>
        </button>
        <button type="button" data-testid="vm-mode-btn" class:sel={wMode === 'iso'} class={cn('flex flex-col items-start gap-1 p-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-left cursor-pointer', wMode === 'iso' && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')} onclick={() => setMode('iso')}>
          <IconDeviceDesktop size={20} stroke={1.7} />
          <span class="vm-mode-t text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('vm.mode_iso')}</span>
          <span class="vm-mode-h text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.mode_iso_hint')}</span>
        </button>
      </div>

      {#if wMode === 'catalogue'}
        <fieldset class="vm-field border-0 p-0 m-0">
          <legend class="mb-1.5 text-xs font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.w_os')}</legend>
          <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-2">
            {#each osOptions as o (o.id)}
              {@const bk = `vm.os_${o.id.replace(/[.\-]/g, '_')}`}
              <button type="button" data-testid="vm-os" class:sel={wOs === o.id} class={cn('flex flex-col items-start gap-0.5 p-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-left cursor-pointer', wOs === o.id && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')} onclick={() => pickOs(o.id)}>
                <span class="vm-os-label text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{o.label}{#if o.recommended}<span data-testid="vm-os-reco" class="ml-1 px-1.5 py-0.5 rounded bg-[var(--hv-success-bg)] text-[var(--hv-success)] text-micro font-bold uppercase">{$t('vm.w_recommended')}</span>{/if}</span>
                <span class="vm-os-blurb text-c-xs text-[var(--hv-shell-text-2)]">{ $t(bk) === bk ? o.blurb : $t(bk)}</span>
              </button>
            {/each}
          </div>
        </fieldset>
      {:else}
        <fieldset class="vm-field border-0 p-0 m-0">
          <legend class="mb-1.5 text-xs font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.w_iso_os')}</legend>
          <div data-testid="vm-kind-toggle" class="flex gap-1.5 p-1.5 rounded-xl bg-[var(--hv-shell-border-soft)]">
            <button type="button" class:sel={wIsoOs === 'linux'} class={cn(vmKindToggleBtn, wIsoOs === 'linux' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} onclick={() => setIsoOs('linux')}>{$t('vm.iso_linux')}</button>
            <button type="button" class:sel={wIsoOs === 'windows'} class={cn(vmKindToggleBtn, wIsoOs === 'windows' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} onclick={() => setIsoOs('windows')}>{$t('vm.iso_windows')}</button>
          </div>
          <span class={vmFieldHint}>{wIsoOs === 'windows' ? $t('vm.iso_windows_hint') : $t('vm.iso_linux_hint')}</span>
        </fieldset>

        <div class={vmField}>
          <span class={vmFieldLabel}>{$t('vm.w_iso_file')}</span>
          <PathPickerField bind:value={wIsoPath} mode="file" ariaLabel={$t('vm.w_iso_file')} />
          {#if isoError}<span data-testid="vm-field-err" class={vmFieldErr}>{isoError}</span>{:else}<span class={vmFieldHint}>{$t('vm.w_iso_file_hint')}</span>{/if}
          {#if storeIsos.length}
            <div class="flex flex-wrap gap-1.5">
              <span class="text-c-xs font-semibold text-[var(--hv-shell-text-2)]">{$t('vm.iso_store')}</span>
              {#each storeIsos as iso (iso.name)}
                <span class:sel={wIsoPath === iso.path} class={cn('inline-flex items-center gap-1 px-2 py-0.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-c-xs', wIsoPath === iso.path && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')}>
                  <button type="button" class="border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-strong)]" onclick={() => wIsoPath = iso.path}>{iso.name} <span class="vm-muted text-[var(--hv-shell-text-2)]">{fmtBytes(iso.size)}</span></button>
                  <button type="button" class="vm-iso-chip-x border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-2)] hover:text-[var(--hv-error)]" onclick={() => deleteIso(iso.name)} aria-label={$t('vm.delete')}><IconX size={12} stroke={2.5} /></button>
                </span>
              {/each}
            </div>
          {/if}
          <div class="vm-iso-url">
            {#if isoUrlOpen}
              <div class="flex gap-2">
                <input type="url" class={textInput} bind:value={isoUrl} placeholder="https://…/ubuntu-24.04.iso" spellcheck="false" autocapitalize="off" />
                <Button variant="primary" size="sm" disabled={isoBusy || !/^https:\/\/.+\.iso(\?.*)?$/i.test(isoUrl.trim())} onclick={downloadIso}>{#if isoBusy}<IconLoader2 class="vm-spin" size={14} stroke={2} />{:else}{$t('vm.iso_get')}{/if}</Button>
                <Button variant="secondary" size="sm" onclick={() => isoUrlOpen = false}>{$t('common.cancel')}</Button>
              </div>
            {:else}
              <button type="button" class="vm-linkbtn border-none bg-transparent text-xs font-semibold text-[var(--hv-primary)] cursor-pointer hover:underline" onclick={() => isoUrlOpen = true}>{$t('vm.iso_from_url')}</button>
            {/if}
          </div>
        </div>

        {#if wIsoOs === 'windows'}
          <div class={vmField}>
            <span class={vmFieldLabel}>{$t('vm.w_drivers_iso')}</span>
            <PathPickerField bind:value={wDriversIso} mode="file" ariaLabel={$t('vm.w_drivers_iso')} />
            <div class="vm-iso-url">
              {#if virtioIso}
                <button type="button" class="vm-linkbtn border-none bg-transparent text-xs font-semibold text-[var(--hv-primary)] cursor-pointer hover:underline" onclick={() => wDriversIso = virtioIso!.path}>{$t('vm.use_virtio', { name: virtioIso.name })}</button>
              {:else}
                <button type="button" class="vm-linkbtn border-none bg-transparent text-xs font-semibold text-[var(--hv-primary)] cursor-pointer hover:underline" disabled={isoBusy} onclick={downloadVirtio}>{#if isoBusy}{$t('vm.iso_get')}…{:else}{$t('vm.get_virtio')}{/if}</button>
              {/if}
            </div>
            <span class={vmFieldHint}>{$t('vm.w_drivers_iso_hint')}</span>
          </div>
        {/if}
      {/if}

      <label class={vmField}>
        <span class={vmFieldLabel}>{$t('vm.w_name')}</span>
        <input type="text" class={textInput} bind:value={wName} oninput={() => wNameTouched = true} placeholder="ubuntu-1" autocapitalize="off" autocomplete="off" spellcheck="false" />
        {#if nameError}<span data-testid="vm-field-err" class={vmFieldErr}>{nameError}</span>{:else}<span class={vmFieldHint}>{$t('vm.w_name_hint')}</span>{/if}
      </label>

      <fieldset class="vm-field border-0 p-0 m-0">
        <legend class="mb-1.5 text-xs font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.w_size')}</legend>
        <div class="grid grid-cols-3 gap-2">
          {#each ['small', 'medium', 'large'] as s (s)}
            <button type="button" data-testid="vm-size" class:sel={wSize === s} class={cn('flex flex-col items-start gap-0.5 p-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-left cursor-pointer', wSize === s && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')} onclick={() => applySize(s as 'small' | 'medium' | 'large')}>
              <span class="vm-size-name text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t(`vm.w_size_${s}`)}</span>
              <span class="vm-size-spec text-micro text-[var(--hv-shell-text-2)]">{SIZE_SPECS[s].cpu} {$t('vm.vcpu')} · {fmtMem(SIZE_SPECS[s].mem)} · {fmtDisk(SIZE_SPECS[s].disk)}</span>
            </button>
          {/each}
        </div>
        {#if wMode === 'iso' && wIsoOs === 'windows'}<span class={vmFieldHint}>{$t('vm.w_size_windows_hint')}</span>{/if}
      </fieldset>

      {#if wMode === 'catalogue'}
        <fieldset class="vm-field border-0 p-0 m-0">
          <legend class="mb-1.5 text-xs font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.w_access')}</legend>
          <p class={vmFieldHint}>{$t('vm.w_access_hint')}</p>
          <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-2">
            <label class="flex flex-col gap-1 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.w_username')}<input type="text" class={textInput} bind:value={wUser} autocapitalize="off" autocomplete="off" spellcheck="false" /></label>
            <label class="flex flex-col gap-1 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.w_password')}
              <span data-testid="vm-pass" class="relative">
                {#if wShowPass}<input type="text" class={textInput} bind:value={wPass} autocomplete="new-password" />{:else}<input type="password" class={textInput} bind:value={wPass} autocomplete="new-password" />{/if}
                <button type="button" data-testid="vm-btn-icon" class={cn(actionIconBtn, 'absolute right-1 top-1 h-9 w-9 hv-ic-abs')} onclick={() => wShowPass = !wShowPass} aria-label={wShowPass ? $t('vm.w_hide') : $t('vm.w_show')}>{#if wShowPass}<IconEyeOff size={15} stroke={1.9} />{:else}<IconEye size={15} stroke={1.9} />{/if}</button>
              </span>
            </label>
          </div>
          {#if passError}<span data-testid="vm-field-err" class={vmFieldErr}>{passError}</span>{/if}
        </fieldset>
      {:else}
        <p data-testid="vm-iso-note" class="vm-iso-note m-0 text-c-xs text-[var(--hv-shell-text-2)]">{$t('vm.iso_account_note')}</p>
      {/if}

      <button type="button" data-testid="vm-adv-toggle" class="vm-adv-toggle flex items-center gap-1.5 border-none bg-transparent text-xs font-semibold text-[var(--hv-shell-text-strong)] cursor-pointer" onclick={() => wAdvanced = !wAdvanced} aria-expanded={wAdvanced}>
        {#if wAdvanced}<IconChevronDown size={15} stroke={2} />{:else}<IconChevronRight size={15} stroke={2} />{/if}
        {$t('vm.w_advanced')}
      </button>
      {#if wAdvanced}
        <div data-testid="vm-adv" class="vm-adv flex flex-col gap-3 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
          {#if wMode === 'catalogue'}
            <fieldset class="vm-field border-0 p-0 m-0">
              <legend class="mb-1.5 text-xs font-semibold text-[var(--hv-shell-text-strong)]">{$t('vm.w_kind')}</legend>
              <div data-testid="vm-kind-toggle" class="flex gap-1.5 p-1.5 rounded-xl bg-[var(--hv-shell-glass)]">
                <button type="button" class:sel={wKind === 'vm'} class={cn(vmKindToggleBtn, wKind === 'vm' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} onclick={() => { wKind = 'vm'; if (selectedOs && !selectedOs.kinds.includes('vm')) pickOs(osForKind[0]?.id ?? wOs); }}>{$t('vm.kind_vm')}</button>
                <button type="button" class:sel={wKind === 'container'} class={cn(vmKindToggleBtn, wKind === 'container' && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} onclick={() => { wKind = 'container'; if (selectedOs && !selectedOs.kinds.includes('container')) pickOs(osForKind[0]?.id ?? wOs); }}>{$t('vm.kind_container')}</button>
              </div>
              <span class={vmFieldHint}>{$t('vm.w_kind_hint')}</span>
            </fieldset>
          {:else if wIsoOs !== 'windows'}
            <label class="vm-check flex items-center gap-1.5 text-xs text-[var(--hv-shell-text-2)] cursor-pointer"><input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={wSecureBoot} /> {$t('vm.w_secure_boot')}</label>
            <span class={vmFieldHint}>{$t('vm.w_secure_boot_hint')}</span>
          {/if}
          <div class="grid grid-cols-3 gap-2">
            <label class="flex flex-col gap-1 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.vcpu')}<input type="number" class={textInput} min="1" max="64" bind:value={wCpu} /></label>
            <label class="flex flex-col gap-1 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.memory')} ({$t('vm.unit_mb')})<input type="number" class={textInput} min="256" max="131072" step="256" bind:value={wMem} /></label>
            <label class="flex flex-col gap-1 text-xs text-[var(--hv-shell-text-2)]">{$t('vm.disk')} ({$t('vm.unit_gb')})<input type="number" class={textInput} min="5" max="4096" bind:value={wDisk} /></label>
          </div>
        </div>
      {/if}
    </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" data-testid="vm-btn" onclick={() => wizardOpen = false}>{$t('common.cancel')}</Button>
        <Button variant="primary" data-testid="vm-btn-primary" disabled={!canCreate} onclick={submitWizard}>{#if wBusy}<IconLoader2 class="vm-spin" size={16} stroke={2} />{:else}{$t('vm.wizard_create')}{/if}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

{#if snapOpen}
  <Modal open={snapOpen} title={$t('vm.snap_new')} size="md" onclose={() => snapOpen = false}>
    {#snippet children()}
      <label class={vmField}><span class={vmFieldLabel}>{$t('vm.snap_name')}</span><input type="text" class={textInput} bind:value={snapName} placeholder="avant-mise-a-jour" autocapitalize="off" spellcheck="false" /><span class={vmFieldHint}>{$t('vm.snap_name_hint')}</span></label>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" onclick={() => snapOpen = false}>{$t('common.cancel')}</Button>
        <Button variant="primary" disabled={!snapName.trim() || snapBusy} onclick={createSnap}>{#if snapBusy}<IconLoader2 class="vm-spin" size={16} stroke={2} />{:else}{$t('vm.snap_save')}{/if}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

{#if confirmDelete}
  {@const cd = confirmDelete}
  <Modal open={!!cd} title={$t('vm.delete_machine')} size="md" onclose={() => confirmDelete = null}>
    {#snippet icon()}<IconAlertTriangle size={18} stroke={2} />{/snippet}
    {#snippet children()}<p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('vm.delete_confirm', { name: cd })}</p>{/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" onclick={() => confirmDelete = null}>{$t('common.cancel')}</Button>
        <Button variant="danger" data-testid="vm-btn-primary" onclick={() => doDelete(cd)}><IconTrash size={15} stroke={1.9} /> {$t('vm.delete')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

{#if confirmRestore}
  {@const cr = confirmRestore}
  <Modal open={!!cr} title={$t('vm.snap_restore')} size="md" onclose={() => confirmRestore = null}>
    {#snippet icon()}<IconAlertTriangle size={18} stroke={2} />{/snippet}
    {#snippet children()}<p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('vm.restore_confirm', { name: cr.name })}</p>{/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" onclick={() => confirmRestore = null}>{$t('common.cancel')}</Button>
        <Button variant="primary" onclick={() => restoreSnap(cr)}><IconArrowBackUp size={15} stroke={1.9} /> {$t('vm.snap_restore')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}
</div>

{#if consoleVm}
  <VmConsole vmName={consoleVm} onClose={() => consoleVm = null} />
{/if}

{#if screenVm}
  <VmScreen vmName={screenVm} onClose={() => screenVm = null} />
{/if}