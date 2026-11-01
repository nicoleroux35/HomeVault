<!-- ========================================================================
     File          : SecurityAdvisorCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The Health screen's Security Advisor (QW-10): one read-only answer to
       "is this box safely configured?" — a weighted score, the findings that
       need action with the sentence that fixes them, and the full checklist
       behind a disclosure. Every finding deep-links to the screen that owns
       the setting; the card itself never changes anything.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/stores/settingsCatalog, theme/Button, theme/Spinner

     Usage:
       Embedded at the top of routes/Health.svelte (admin sessions only — the
       report is the security posture of the appliance). See
       docs/guides/health.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { requestSettingsCategory } from '$lib/stores/settingsCatalog';
  import { Button, Spinner } from '$lib/components/theme';
  import {
    IconShieldCheck,
    IconAlertTriangle,
    IconCircleCheck,
    IconInfoCircle
  } from '@tabler/icons-svelte';

  type Severity = 'pass' | 'info' | 'warn' | 'fail';

  interface DeepLink {
    app: string;
    category?: string | null;
  }

  interface Finding {
    id: string;
    severity: Severity;
    title: string;
    evidence: string;
    fix?: string | null;
    link?: DeepLink | null;
  }

  interface Counts {
    pass: number;
    info: number;
    warn: number;
    fail: number;
    total: number;
  }

  interface Report {
    score: number;
    counts: Counts;
    findings: Finding[];
    generated_at: string;
    app_version: string;
  }

  let loading = $state(true);
  let running = $state(false);
  let error = $state('');
  let report = $state<Report | null>(null);

  // Worst first — the server already sorts, but the card never trusts an
  // ordering it did not ask for: the first line is what the user must fix.
  const actionable = $derived(
    report
      ? report.findings.filter((f) => f.severity === 'warn' || f.severity === 'fail')
      : []
  );
  const settled = $derived(
    report
      ? report.findings.filter((f) => f.severity === 'pass' || f.severity === 'info')
      : []
  );

  async function load(initial = false) {
    if (initial) loading = true;
    else running = true;
    error = '';
    const response = await api.getSecurityAdvisor();
    if (response.success && response.data) {
      report = response.data as unknown as Report;
    } else {
      // Never keep a stale score on screen when the new run failed: an old
      // "100" beside a network error is the one misleading answer possible.
      report = null;
      error = response.message || $t('health.advisor_error');
    }
    loading = false;
    running = false;
  }

  onMount(() => load(true));

  function severityColor(severity: Severity): string {
    switch (severity) {
      case 'pass':
        return 'var(--hv-success)';
      case 'warn':
        return 'var(--hv-warning)';
      case 'fail':
        return 'var(--hv-error)';
      default:
        return 'var(--hv-info)';
    }
  }

  function severityLabel(severity: Severity): string {
    switch (severity) {
      case 'pass':
        return $t('health.advisor_pass');
      case 'warn':
        return $t('health.advisor_warn');
      case 'fail':
        return $t('health.advisor_fail');
      default:
        return $t('health.advisor_info');
    }
  }

  function scoreColor(score: number): string {
    if (score >= 90) return 'var(--hv-success)';
    if (score >= 60) return 'var(--hv-warning)';
    return 'var(--hv-error)';
  }

  // The link is a deep link into an existing screen: the desktop owns the
  // window, and Settings reads the requested category on its next render.
  //
  // `category` travels with the event as well as through the store. The store
  // is the one-shot channel the desktop's tooltip paths use, but it is
  // consumed by whichever Settings instance renders next; carrying the value
  // in the event too means a click from this card always lands on the right
  // category, even if a stale request was left pending by an earlier one.
  function openFinding(finding: Finding) {
    const link = finding.link;
    if (!link?.app) return;
    const category = link.app === 'settings' ? (link.category ?? null) : null;
    if (category) {
      requestSettingsCategory(category);
    }
    // `source` marks the exact request this card sent, so a test (or a log
    // reader) can tell it apart from any other window open.
    window.dispatchEvent(
      new CustomEvent('hv-open-app', {
        detail: { id: link.app, category, source: 'security-advisor' }
      })
    );
  }

  function linkLabel(finding: Finding): string {
    const app = finding.link?.app;
    const key = app ? `health.advisor_open_${app}` : '';
    const translated = key ? $t(key) : '';
    return translated && translated !== key ? translated : $t('health.advisor_open');
  }
</script>

<section
  data-testid="advisor-card"
  aria-labelledby="advisor-heading"
  class="rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] p-5 mb-5"
>
  <div class="flex flex-wrap items-center gap-3 mb-4">
    <span
      class="grid place-items-center w-11 h-11 rounded-xl flex-none"
      style="background: var(--hv-success-light, rgba(30,126,52,0.12)); color: var(--hv-success)"
    >
      <IconShieldCheck size={22} />
    </span>
    <div class="min-w-0 flex-1">
      <h2 id="advisor-heading" class="m-0 text-c-md font-semibold text-foreground">
        {$t('health.advisor_title')}
      </h2>
      <p class="m-0 text-c-sm text-muted-foreground">{$t('health.advisor_subtitle')}</p>
    </div>
    <Button
      variant="secondary"
      data-testid="advisor-rerun"
      loading={running}
      onclick={() => load()}
    >
      {$t('health.advisor_rerun')}
    </Button>
  </div>

  {#if loading}
    <div
      data-testid="advisor-loading"
      class="flex items-center gap-3 py-6 text-muted-foreground"
    >
      <Spinner />
      <span class="text-c-base">{$t('health.advisor_loading')}</span>
    </div>
  {:else if error}
    <p data-testid="advisor-error" role="alert" class="m-0 text-c-base" style="color: var(--hv-error)">
      {error}
    </p>
  {:else if report}
    <!-- Score. A real reading, so it is a progressbar with a name and a value
         — not a decorative number. -->
    <div class="flex flex-wrap items-center gap-4 mb-4">
      <div class="min-w-40 flex-1">
        <div class="flex items-baseline justify-between gap-3 mb-1.5">
          <span class="text-c-base font-semibold text-foreground">
            {$t('health.advisor_score')}
          </span>
          <span
            data-testid="advisor-score"
            class="text-c-lg font-bold"
            style="color: {scoreColor(report.score)}"
          >
            {$t('health.advisor_score_value', { score: String(report.score) })}
          </span>
        </div>
        <div
          class="h-2.5 w-full rounded-full overflow-hidden"
          style="background: var(--hv-border, #E5E7EB)"
          role="progressbar"
          aria-labelledby="advisor-heading"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={report.score}
          aria-valuetext={$t('health.advisor_score_value', { score: String(report.score) })}
        >
          <div
            class="h-full rounded-full"
            style="width: {report.score}%; background: {scoreColor(report.score)}"
          ></div>
        </div>
        <p data-testid="advisor-summary" class="m-0 mt-2 text-c-sm text-muted-foreground">
          {$t('health.advisor_summary', {
            pass: String(report.counts.pass),
            warn: String(report.counts.warn),
            fail: String(report.counts.fail)
          })}
        </p>
      </div>
    </div>

    {#if actionable.length === 0}
      <p
        data-testid="advisor-nothing-to-fix"
        class="m-0 flex items-center gap-2 text-c-base"
        style="color: var(--hv-success)"
      >
        <IconCircleCheck size={18} />
        {$t('health.advisor_nothing_to_fix')}
      </p>
    {:else}
      <h3 class="m-0 mb-2 text-c-base font-semibold text-foreground">
        {$t('health.advisor_fix_first')}
      </h3>
      <ul data-testid="advisor-findings" class="m-0 p-0 list-none flex flex-col gap-2.5">
        {#each actionable as finding (finding.id)}
          <li
            data-testid="advisor-finding"
            data-severity={finding.severity}
            data-finding-id={finding.id}
            class="rounded-lg border border-border p-3"
          >
            <div class="flex flex-wrap items-center gap-2 mb-1">
              <span
                class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-c-xs font-semibold"
                style="background: {severityColor(finding.severity)}15; color: {severityColor(finding.severity)}"
              >
                {#if finding.severity === 'fail'}
                  <IconAlertTriangle size={12} />
                {:else}
                  <IconInfoCircle size={12} />
                {/if}
                {severityLabel(finding.severity)}
              </span>
              <span data-testid="advisor-finding-title" class="text-c-base font-semibold text-foreground">
                {finding.title}
              </span>
            </div>
            {#if finding.fix}
              <p class="m-0 text-c-sm text-muted-foreground">{finding.fix}</p>
            {/if}
            {#if finding.link?.app}
              <div class="mt-2">
                <Button
                  variant="secondary"
                  size="sm"
                  data-testid="advisor-open"
                  onclick={() => openFinding(finding)}
                >
                  {linkLabel(finding)}
                </Button>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <!-- The rest of the checklist: a native disclosure, so it is keyboard
         reachable and screen-reader announced without extra script. -->
    <details class="mt-4" data-testid="advisor-all">
      <summary class="cursor-pointer text-c-sm font-semibold text-foreground">
        {$t('health.advisor_all_checks', { count: String(settled.length) })}
      </summary>
      <ul class="m-0 mt-2 p-0 list-none flex flex-col gap-1.5">
        {#each settled as finding (finding.id)}
          <li
            data-testid="advisor-check"
            data-finding-id={finding.id}
            class="flex flex-wrap items-baseline gap-2 text-c-sm"
          >
            <span class="font-medium" style="color: {severityColor(finding.severity)}">
              {severityLabel(finding.severity)}
            </span>
            <span class="text-foreground">{finding.title}</span>
            <span class="text-muted-foreground">— {finding.evidence}</span>
          </li>
        {/each}
      </ul>
    </details>
  {/if}
</section>
