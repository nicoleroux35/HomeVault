<!-- ========================================================================
     File          : TrafficControl.svelte
     Author        : Nicolas Le Roux
     Created       : 01/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Per-service bandwidth control (ROADMAP P3.2). One egress queue on the
     interface the admin picks, one rate per protocol server, an optional
     weekly schedule and a pause-all switch. Management ports (SSH, HTTPS) are
     never shaped, so an admin cannot throttle their own session out.

     Dependencies:
       - svelte (>= 5), theme + app composites, $lib/services/api

     Usage:
       Registered as the "Traffic control" desktop app (admin only).
     ======================================================================== -->
<script lang="ts">
  /**
   * TrafficControl — per-service bandwidth shaping.
   *
   * The screen edits one policy object and saves it as a whole: services are
   * `{ service, rate_kbps }` rows where `0` means unlimited, and a schedule
   * is a list of weekly windows. The backend does the validation and the
   * `tc` invocation; this page only mirrors the contract and reports the
   * apply result.
   *
   * Accessibility: the status line is a `role="status"` live region, weekday
   * pickers are toggle buttons with `aria-pressed`, every rate input has a
   * visible label, and the compact delete button gets the shared 44px `.hv-ic`
   * hit square.
   */
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button, Toggle, Alert } from '$lib/components/theme';
  import { PageLayout, SectionCard } from '$lib/components/app';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { cn } from '$lib/utils.js';
  import {
    IconGauge,
    IconCheck,
    IconPlus,
    IconTrash,
    IconPlayerPause,
    IconPlayerPlay,
    IconRefresh,
    IconInfoCircle,
  } from '@tabler/icons-svelte';

  interface ServiceCapability {
    id: string;
    ports: number[];
    supports_upload_limit: boolean;
    supports_download_limit: boolean;
    note: string;
  }

  interface Capabilities {
    tc_available: boolean;
    ifb_available: boolean;
    services: ServiceCapability[];
    management_ports: number[];
  }

  interface NetworkInterface {
    name: string;
    type: string;
    status: string;
    ipv4: string | null;
  }

  interface ScheduleWindow {
    days: number[];
    start_minute: number;
    end_minute: number;
  }

  interface Settings {
    enabled: boolean;
    interface: string;
    paused: boolean;
    services: { service: string; rate_kbps: number }[];
    schedule: ScheduleWindow[];
    updated_at: string;
  }

  interface TrafficState {
    shaping: boolean;
    reason: string | null;
    reason_code: string | null;
    commands: string[];
  }

  interface ApplyReport {
    applied: boolean;
    reason: string | null;
    commands: string[];
    failed: string | null;
  }

  let settings = $state<Settings>({
    enabled: false,
    interface: '',
    paused: false,
    services: [],
    schedule: [],
    updated_at: '',
  });
  let capabilities = $state<Capabilities | null>(null);
  let interfaces = $state<NetworkInterface[]>([]);
  let liveState = $state<TrafficState>({ shaping: false, reason: null, reason_code: 'off', commands: [] });
  let lastApply = $state<ApplyReport | null>(null);

  let loading = $state(true);
  let busy = $state<'save' | 'pause' | 'apply' | null>(null);
  let error = $state('');

  /** Rate per service id, in kbit/s (`0` = unlimited). */
  let rates = $state<Record<string, number>>({});
  const DAY_KEYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] as const;

  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const hint = 'm-0 text-c-label text-[var(--hv-shell-text-2)]';
  const inp = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-focus-ring)]';

  onMount(load);

  async function load() {
    loading = true;
    error = '';
    try {
      const resp = await api.get('/network/traffic-control');
      if (resp.success && resp.data) {
        applyPayload(resp.data);
      } else {
        error = resp.message || translate('traffic.load_failed');
      }
    } catch {
      error = translate('traffic.load_failed');
    } finally {
      loading = false;
    }
  }

  function applyPayload(data: any) {
    settings = data.settings as Settings;
    capabilities = data.capabilities as Capabilities;
    interfaces = data.interfaces as NetworkInterface[];
    liveState = data.state as TrafficState;
    if (data.apply) lastApply = data.apply as ApplyReport;
    const next: Record<string, number> = {};
    for (const cap of capabilities?.services ?? []) next[cap.id] = 0;
    for (const row of settings.services) next[row.service] = row.rate_kbps;
    rates = next;
  }

  /** Services in the payload, always all known ids — `0` is a valid value. */
  function servicesPayload() {
    return Object.entries(rates).map(([service, rate_kbps]) => ({ service, rate_kbps }));
  }

  function body() {
    return {
      enabled: settings.enabled,
      interface: settings.interface,
      paused: settings.paused,
      services: servicesPayload(),
      schedule: settings.schedule.map((w) => ({
        days: [...w.days].sort((a, b) => a - b),
        start_minute: w.start_minute,
        end_minute: w.end_minute,
      })),
    };
  }

  async function save() {
    busy = 'save';
    error = '';
    try {
      const resp = await api.put('/network/traffic-control', body());
      if (resp.success && resp.data) {
        applyPayload(resp.data);
        notifySuccess(reportMessage(resp.data.apply));
      } else {
        error = resp.message || translate('traffic.save_failed');
        notifyError(resp);
      }
    } catch {
      error = translate('traffic.save_failed');
    } finally {
      busy = null;
    }
  }

  async function setPaused(paused: boolean) {
    busy = 'pause';
    try {
      const resp = await api.post('/network/traffic-control/pause', { paused });
      if (resp.success && resp.data) {
        applyPayload({ ...resp.data, capabilities, interfaces });
        notifySuccess(paused ? 'traffic.paused_done' : 'traffic.resumed_done');
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('traffic.save_failed') });
    } finally {
      busy = null;
    }
  }

  async function applyNow() {
    busy = 'apply';
    try {
      const resp = await api.post('/network/traffic-control/apply');
      if (resp.success && resp.data) {
        applyPayload({ ...resp.data, capabilities, interfaces });
        notifySuccess(reportMessage(resp.data.apply));
      } else {
        notifyError(resp);
      }
    } catch {
      notifyError({ message: translate('traffic.save_failed') });
    } finally {
      busy = null;
    }
  }

  function reportMessage(report: ApplyReport | null): string {
    if (!report) return 'common.saved';
    if (report.failed) return 'traffic.apply_failed';
    return report.applied ? 'traffic.applied_on' : 'traffic.applied_off';
  }

  function reasonText(code: string | null): string {
    if (!code) return translate('traffic.reason_active');
    const key = `traffic.reason_${code}`;
    const text = translate(key);
    return text === key ? translate('traffic.reason_active') : text;
  }

  function serviceLabel(id: string): string {
    const key = `traffic.service_${id}`;
    const text = translate(key);
    return text === key ? id.toUpperCase() : text;
  }

  /** kbit/s → a human "≈ X MB/s" note; `0` stays "unlimited". */
  function rateHint(kbps: number): string {
    if (!kbps || kbps <= 0) return translate('traffic.unlimited');
    const mbps = kbps / 8 / 1024;
    return translate('traffic.rate_equiv', { rate: mbps >= 10 ? mbps.toFixed(0) : mbps.toFixed(1) });
  }

  function minutesToTime(minutes: number): string {
    const h = Math.floor(minutes / 60) % 24;
    const m = minutes % 60;
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
  }

  function timeToMinutes(value: string): number {
    const [h, m] = value.split(':').map(Number);
    if (Number.isNaN(h) || Number.isNaN(m)) return 0;
    return Math.min(1439, Math.max(0, h * 60 + m));
  }

  function addWindow() {
    settings.schedule = [
      ...settings.schedule,
      { days: [0, 1, 2, 3, 4], start_minute: 9 * 60, end_minute: 17 * 60 },
    ];
  }

  function removeWindow(index: number) {
    settings.schedule = settings.schedule.filter((_, i) => i !== index);
  }

  function toggleDay(index: number, day: number) {
    const window = settings.schedule[index];
    const days = window.days.includes(day)
      ? window.days.filter((d) => d !== day)
      : [...window.days, day];
    settings.schedule = settings.schedule.map((w, i) => (i === index ? { ...w, days } : w));
  }

  function windowSummary(window: ScheduleWindow): string {
    const days = [...window.days].sort((a, b) => a - b).map((d) => translate(`traffic.day_${DAY_KEYS[d]}`));
    const range = `${minutesToTime(window.start_minute)}–${minutesToTime(window.end_minute)}`;
    return days.length ? `${days.join(', ')} · ${range}` : translate('traffic.window_no_days');
  }
</script>

<PageLayout title={$t('traffic.title')} data-testid="traffic-page">
  {#snippet icon()}
    <IconGauge stroke={1.8} />
  {/snippet}

  {#snippet toolbarRight()}
    <div class="flex items-center gap-2">
      <Button variant="secondary" data-testid="traffic-apply" disabled={busy !== null} onclick={applyNow}>
        <IconRefresh size={16} stroke={2} /> {$t('traffic.apply_now')}
      </Button>
      <Button variant="primary" data-testid="traffic-save" loading={busy === 'save'} disabled={busy !== null} onclick={save}>
        <IconCheck size={16} stroke={2} /> {$t('traffic.save')}
      </Button>
    </div>
  {/snippet}

  {#if loading}
    <div data-testid="traffic-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('common.loading')}</span>
    </div>
  {:else}
    <div data-testid="traffic-content" class="flex flex-col gap-4 max-w-[880px]">
      {#if error}
        <Alert variant="error">{error}</Alert>
      {/if}

      <!-- Live status. `aria-live` is polite: saving flips it and the admin
           hears the result without the page stealing focus. -->
      <div
        data-testid="traffic-status"
        role="status"
        class={cn(
          'flex items-start gap-3 p-4 rounded-xl border',
          liveState.shaping
            ? 'bg-[var(--hv-success-bg)] border-[var(--hv-success)]'
            : 'bg-[var(--hv-shell-border-soft)] border-[var(--hv-shell-border)]',
        )}
      >
        {#if liveState.shaping}
          <IconCheck size={20} stroke={2} class="flex-none text-[var(--hv-success)]" aria-hidden="true" />
        {:else}
          <IconInfoCircle size={20} stroke={2} class="flex-none text-[var(--hv-shell-text-2)]" aria-hidden="true" />
        {/if}
        <div class="flex flex-col gap-0.5 min-w-0">
          <span class="font-semibold text-[var(--hv-shell-text-strong)]">
            {liveState.shaping ? $t('traffic.status_on') : $t('traffic.status_off')}
          </span>
          <span class="text-c-label text-[var(--hv-shell-text-2)]">
            {reasonText(liveState.reason_code)}
          </span>
          {#if capabilities && !capabilities.tc_available}
            <span class="text-c-label text-[var(--hv-warning)]" data-testid="traffic-tc-missing">
              {$t('traffic.tc_missing')}
            </span>
          {/if}
          {#if lastApply?.failed}
            <span class="text-c-label text-[var(--hv-danger)]" role="alert" data-testid="traffic-apply-error">
              {lastApply.failed}
            </span>
          {/if}
        </div>
      </div>

      <SectionCard title={$t('traffic.policy_title')} description={$t('traffic.policy_desc')} data-testid="traffic-policy">
        <div class="flex flex-col gap-4">
          <Toggle
            bind:checked={settings.enabled}
            label={$t('traffic.enabled')}
            description={$t('traffic.enabled_hint')}
          />

          <label class="flex flex-col gap-1.5">
            <span class={fieldLabel}>{$t('traffic.interface')}</span>
            <select class={inp} bind:value={settings.interface} data-testid="traffic-interface" disabled={!settings.enabled}>
              <option value="">{$t('traffic.interface_none')}</option>
              {#each interfaces as iface (iface.name)}
                <option value={iface.name}>
                  {iface.name}{iface.ipv4 ? ` — ${iface.ipv4}` : ''}
                </option>
              {/each}
            </select>
            <span class={hint}>{$t('traffic.interface_hint')}</span>
          </label>

          {#if settings.enabled}
            <div class="flex flex-col gap-2">
              <span class={fieldLabel}>{$t('traffic.services_title')}</span>
              <p class={hint}>{$t('traffic.services_hint')}</p>
              <div class="flex flex-col gap-2">
                {#each capabilities?.services ?? [] as cap (cap.id)}
                  <div class="flex items-center justify-between gap-3 flex-wrap p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]" data-testid="traffic-service-{cap.id}">
                    <div class="flex flex-col min-w-0">
                      <span class="font-semibold text-[var(--hv-shell-text-strong)]">{serviceLabel(cap.id)}</span>
                      <span class="text-c-label text-[var(--hv-shell-text-2)]">
                        {$t('traffic.ports', { ports: cap.ports.join(', ') })}
                      </span>
                    </div>
                    <label class="flex items-center gap-2">
                      <span class="sr-only">{$t('traffic.rate_for', { service: serviceLabel(cap.id) })}</span>
                      <input
                        class="w-28 h-11 px-3 text-right border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-bg)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-focus-ring)]"
                        type="number"
                        min="0"
                        step="100"
                        bind:value={rates[cap.id]}
                        data-testid="traffic-rate-{cap.id}"
                      />
                      <span class="text-c-label text-[var(--hv-shell-text-2)]">{rateHint(rates[cap.id] ?? 0)}</span>
                    </label>
                  </div>
                {/each}
              </div>
              <p class={hint}>{$t('traffic.egress_note')}</p>
            </div>
          {/if}
        </div>
      </SectionCard>

      <SectionCard title={$t('traffic.schedule_title')} description={$t('traffic.schedule_desc')} data-testid="traffic-schedule">
        {#snippet actions()}
          <Button variant="secondary" size="sm" data-testid="traffic-add-window" disabled={!settings.enabled} onclick={addWindow}>
            <IconPlus size={15} stroke={2} /> {$t('traffic.add_window')}
          </Button>
        {/snippet}
        <div class="flex flex-col gap-3">
          {#if settings.schedule.length === 0}
            <p class={hint} data-testid="traffic-schedule-empty">{$t('traffic.schedule_empty')}</p>
          {:else}
            {#each settings.schedule as window, index (index)}
              <div class="flex flex-col gap-3 p-3 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]" data-testid="traffic-window-{index}">
                <div class="flex items-start justify-between gap-3">
                  <span class="text-c-label text-[var(--hv-shell-text-2)]">{windowSummary(window)}</span>
                  <button
                    type="button"
                    class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic"
                    aria-label={$t('traffic.remove_window')}
                    data-testid="traffic-remove-window-{index}"
                    onclick={() => removeWindow(index)}
                  ><IconTrash size={16} stroke={1.8} /></button>
                </div>
                <div class="flex items-center gap-1 flex-wrap" role="group" aria-label={$t('traffic.days_label')}>
                  {#each DAY_KEYS as dayKey, day}
                    <button
                      type="button"
                      class={cn(
                        'min-w-11 h-11 px-2 rounded-lg border text-c-label font-semibold cursor-pointer focus-visible:shadow-[var(--hv-focus-ring)]',
                        window.days.includes(day)
                          ? 'bg-[var(--hv-primary)] border-[var(--hv-primary)] text-white'
                          : 'bg-transparent border-[var(--hv-shell-border-strong)] text-[var(--hv-shell-text-2)]',
                      )}
                      aria-pressed={window.days.includes(day)}
                      data-testid="traffic-day-{index}-{day}"
                      onclick={() => toggleDay(index, day)}
                    >{$t(`traffic.day_${dayKey}`)}</button>
                  {/each}
                </div>
                <div class="grid gap-3 sm:grid-cols-2">
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{$t('traffic.start')}</span>
                    <input
                      class={inp}
                      type="time"
                      value={minutesToTime(window.start_minute)}
                      data-testid="traffic-start-{index}"
                      oninput={(e) => {
                        const value = timeToMinutes((e.currentTarget as HTMLInputElement).value);
                        settings.schedule = settings.schedule.map((w, i) =>
                          i === index ? { ...w, start_minute: value } : w,
                        );
                      }}
                    />
                  </label>
                  <label class="flex flex-col gap-1.5">
                    <span class={fieldLabel}>{$t('traffic.end')}</span>
                    <input
                      class={inp}
                      type="time"
                      value={minutesToTime(window.end_minute)}
                      data-testid="traffic-end-{index}"
                      oninput={(e) => {
                        const value = timeToMinutes((e.currentTarget as HTMLInputElement).value);
                        settings.schedule = settings.schedule.map((w, i) =>
                          i === index ? { ...w, end_minute: value } : w,
                        );
                      }}
                    />
                  </label>
                </div>
              </div>
            {/each}
          {/if}
        </div>
      </SectionCard>

      <SectionCard title={$t('traffic.pause_title')} description={$t('traffic.pause_desc')} data-testid="traffic-pause-card">
        <div class="flex items-center justify-between gap-3 flex-wrap">
          <span class="text-c-label text-[var(--hv-shell-text-2)]">
            {settings.paused ? $t('traffic.paused_state') : $t('traffic.running_state')}
          </span>
          {#if settings.paused}
            <Button variant="primary" data-testid="traffic-resume" disabled={busy !== null} onclick={() => setPaused(false)}>
              <IconPlayerPlay size={16} stroke={2} /> {$t('traffic.resume')}
            </Button>
          {:else}
            <Button variant="warning" data-testid="traffic-pause" disabled={busy !== null} onclick={() => setPaused(true)}>
              <IconPlayerPause size={16} stroke={2} /> {$t('traffic.pause')}
            </Button>
          {/if}
        </div>
      </SectionCard>
    </div>
  {/if}

  {#snippet footer()}
    {#if !loading}
      <span data-testid="traffic-footer" class="flex items-center gap-2 flex-wrap">
        <span>{settings.interface || $t('traffic.no_interface')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{liveState.shaping ? $t('traffic.status_on') : $t('traffic.status_off')}</span>
      </span>
    {/if}
  {/snippet}
</PageLayout>
