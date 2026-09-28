<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { Spinner, Button } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import { t, translate } from '$lib/stores/i18n';
  import {
    IconAlertTriangle,
    IconCheck,
    IconCircleCheck,
    IconChevronLeft,
    IconChevronRight,
    IconDatabase,
    IconServer,
    IconShieldCheck
  } from '@tabler/icons-svelte';

  interface AvailableDevice {
    name: string;
    size: number;
    model: string;
    inUse: boolean;
  }

  let availableDevices: AvailableDevice[] = [];
  let creating = false;

  // The creation wizard IS the page — RAID only creates "virtual disks";
  // listing / rebuilding / destroying them lives in the Storage app.
  let currentStep = 1;
  let selectedRaidType: string | null = null;
  let raidName = '';
  let selectedDevices: string[] = [];
  let selectedFilesystem = 'ext4';
  let justCreated = '';
  const dlg = createConfirmState();

  $: raidTypes = [
    { id: 'raid0', name: 'RAID 0', description: $t('raid.raid0.desc'), protection: $t('raid.manager.failure_raid0'), minDisks: 2, protects: false, recommended: false },
    { id: 'raid1', name: 'RAID 1', description: $t('raid.raid1.desc'), protection: $t('raid.manager.failure_raid1'), minDisks: 2, protects: true, recommended: false },
    { id: 'raid5', name: 'RAID 5', description: $t('raid.raid5.desc'), protection: $t('raid.manager.failure_raid5'), minDisks: 3, protects: true, recommended: true },
    { id: 'raid6', name: 'RAID 6', description: $t('raid.raid6.desc'), protection: $t('raid.manager.failure_raid6'), minDisks: 4, protects: true, recommended: false },
    { id: 'raid10', name: 'RAID 10', description: $t('raid.raid10.desc'), protection: $t('raid.manager.failure_raid10'), minDisks: 4, protects: true, recommended: false }
  ];

  $: filesystems = [
    { value: 'ext4', label: 'ext4', description: $t('raid.fs.ext4.desc') },
    { value: 'xfs', label: 'XFS', description: $t('raid.fs.xfs.desc') },
    { value: 'btrfs', label: 'Btrfs', description: $t('raid.fs.btrfs.desc') },
    { value: 'zfs', label: 'ZFS', description: $t('raid.fs.zfs.desc') }
  ];

  $: selectedRaid = raidTypes.find((r) => r.id === selectedRaidType) || null;
  $: selectableDevices = availableDevices.filter((d) => !d.inUse);
  $: stepName = currentStep === 1 ? $t('raid.manager.step_type') : currentStep === 2 ? $t('raid.manager.step_disks') : $t('raid.manager.step_configure');

  $: pickedSizes = selectedDevices
    .map((name) => availableDevices.find((d) => d.name === name)?.size || 0)
    .filter((s) => s > 0);
  $: estimatedCapacity = (() => {
    if (!selectedRaidType || pickedSizes.length === 0) return 0;
    const min = Math.min(...pickedSizes);
    const n = pickedSizes.length;
    switch (selectedRaidType) {
      case 'raid0': return pickedSizes.reduce((a, b) => a + b, 0);
      case 'raid1': return min;
      case 'raid5': return min * Math.max(n - 1, 0);
      case 'raid6': return min * Math.max(n - 2, 0);
      case 'raid10': return min * Math.floor(n / 2);
      default: return 0;
    }
  })();

  onMount(async () => {
    await loadAvailableDevices();
  });

  async function loadAvailableDevices() {
    const response = await api.getRaidAvailableDevices();
    if (response.success && response.data) availableDevices = response.data;
  }

  function selectRaidType(typeId: string) {
    selectedRaidType = typeId;
  }

  function nextStep() {
    if (currentStep < 3 && canProceed) currentStep++;
  }

  function prevStep() {
    if (currentStep > 1) currentStep--;
  }

  function toggleDevice(deviceName: string) {
    if (selectedDevices.includes(deviceName)) {
      selectedDevices = selectedDevices.filter((d) => d !== deviceName);
    } else {
      selectedDevices = [...selectedDevices, deviceName];
    }
  }

  function selectAllDevices() {
    selectedDevices = selectableDevices.map((d) => d.name);
  }

  function diskCells(id: string | null): Array<'data' | 'mirror' | 'parity'> {
    switch (id) {
      case 'raid0': return ['data', 'data'];
      case 'raid1': return ['data', 'mirror'];
      case 'raid5': return ['data', 'data', 'parity'];
      case 'raid6': return ['data', 'data', 'parity', 'parity'];
      case 'raid10': return ['data', 'mirror', 'data', 'mirror'];
      default: return [];
    }
  }

  interface DiskColumn { blocks: Array<'data' | 'mirror' | 'parity'>; label: string; }

  function diskColumns(id: string | null): DiskColumn[] {
    switch (id) {
      case 'raid0': return [{ blocks: ['data', 'data'], label: 'D1' }, { blocks: ['data', 'data'], label: 'D2' }];
      case 'raid1': return [{ blocks: ['data', 'data'], label: 'D1' }, { blocks: ['mirror', 'mirror'], label: 'D2' }];
      case 'raid5': return [{ blocks: ['data', 'parity'], label: 'D1' }, { blocks: ['data', 'data'], label: 'D2' }, { blocks: ['parity', 'data'], label: 'D3' }];
      case 'raid6': return [{ blocks: ['data', 'parity'], label: 'D1' }, { blocks: ['data', 'parity'], label: 'D2' }, { blocks: ['parity', 'data'], label: 'D3' }, { blocks: ['parity', 'data'], label: 'D4' }];
      case 'raid10': return [{ blocks: ['data', 'data'], label: 'D1' }, { blocks: ['mirror', 'mirror'], label: 'D2' }, { blocks: ['data', 'data'], label: 'D3' }, { blocks: ['mirror', 'mirror'], label: 'D4' }];
      default: return [];
    }
  }

  // A plain function call in markup (`disabled={!canProceed()}`) isn't reliably
  // re-evaluated by Svelte's template dependency tracking, since the compiler
  // can't see the reactive reads hidden inside the function body. Deriving it
  // as a `$:` value instead makes every dependency explicit and guarantees the
  // button's disabled state updates when the user picks a type/disks/name.
  $: canProceed = currentStep === 1
    ? selectedRaidType !== null
    : currentStep === 2
      ? selectedDevices.length >= (raidTypes.find((r) => r.id === selectedRaidType)?.minDisks || 2)
      : raidName.trim().length > 0;

  async function handleCreate() {
    if (!selectedRaidType || !raidName.trim() || selectedDevices.length === 0) return;
    const raidType = raidTypes.find((r) => r.id === selectedRaidType);
    const devicesList = selectedDevices.join(', ');

    if (!(await dlg.ask(translate('raid.manager.create_confirm', {
      name: raidName, type: raidType?.name || '', disks: devicesList
    })))) return;

    creating = true;
    const response = await api.createRaid({
      name: raidName.trim(),
      level: selectedRaidType,
      devices: selectedDevices,
      filesystem: selectedFilesystem
    });
    creating = false;

    if (response.success) {
      justCreated = raidName.trim();
      currentStep = 1;
      selectedRaidType = null;
      raidName = '';
      selectedDevices = [];
      selectedFilesystem = 'ext4';
      await loadAvailableDevices();
    } else {
      dlg.notify(response.message || response.error || translate('raid.manager.create_failed'));
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i];
  }

  // Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const typeCard =
    'relative text-left bg-[var(--hv-surface-container-lowest)] border-2 border-[var(--hv-border)] rounded-2xl p-4 ' +
    'cursor-pointer flex flex-col gap-2 min-h-[140px] transition-colors hover:border-[var(--hv-border-hover)] ' +
    'focus-visible:outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-shadow-focus)]';
  const diskRow =
    'relative flex items-center gap-3 px-4 py-3 border-2 border-[var(--hv-border)] rounded-xl ' +
    'bg-[var(--hv-surface-container-lowest)] cursor-pointer min-h-[60px] transition-colors hover:border-[var(--hv-border-hover)]';
  const fsCard =
    'text-left bg-[var(--hv-surface-container-lowest)] border-2 border-[var(--hv-border)] rounded-xl p-3 ' +
    'cursor-pointer flex flex-col gap-1 min-h-[62px] transition-colors hover:border-[var(--hv-border-hover)] ' +
    'focus-visible:outline-none focus-visible:border-[var(--hv-primary)] focus-visible:shadow-[var(--hv-shadow-focus)]';
  const raidInput =
    'h-12 px-4 border-2 border-[var(--hv-border)] rounded-xl bg-[var(--hv-surface-container-lowest)] ' +
    'text-[var(--hv-text-primary)] focus:outline-none focus:border-[var(--hv-primary)] focus:shadow-[var(--hv-shadow-focus)]';
  const tinyLabel =
    'text-c-xs font-bold uppercase tracking-[0.05em] text-[var(--hv-text-tertiary)]';
</script>

<PageLayout
  title={$t('raid.manager.header_title')}
  data-testid="raid-page"
>
  {#snippet icon()}
    <IconDatabase stroke={1.8} />
  {/snippet}

  {#if justCreated}
    <div role="status" class="flex items-center gap-2 bg-[var(--hv-success-bg)] text-[var(--hv-success-text)] border border-[var(--hv-success)] rounded-xl px-4 py-3 font-semibold text-c-sm max-w-[660px] mx-auto w-full">
      <IconCircleCheck size={20} class="flex-none" />
      <span>{$t('raid.manager.create_success', { name: justCreated })}</span>
    </div>
  {/if}

  <div class="mx-auto w-full max-w-[660px] flex flex-col gap-3">
    <div data-testid="raid-wizard" class="bg-[var(--hv-surface-container-lowest)] border border-[var(--hv-border)] rounded-[var(--hv-radius-3xl)] shadow-[var(--hv-shadow-md)] w-full flex flex-col overflow-hidden">
      <!-- Stepper -->
      <div class="px-5 pt-5 pb-4 flex flex-col gap-3 border-b border-[var(--hv-border)]">
        <span data-testid="raid-eyebrow" class="text-c-xs font-semibold tracking-[0.04em] text-[var(--hv-text-tertiary)]">
          {$t('raid.manager.step_of', { current: String(currentStep), total: '3' })}
        </span>
        <ol class="list-none m-0 p-0 flex justify-between gap-2">
          {#each [{ n: 1, label: $t('raid.manager.step_type') }, { n: 2, label: $t('raid.manager.step_disks') }, { n: 3, label: $t('raid.manager.step_configure') }] as s}
            <li
              class={cn(
                'flex items-center gap-2 min-w-0 text-[var(--hv-text-tertiary)]',
                currentStep === s.n && 'text-[var(--hv-text-primary)]',
                currentStep > s.n && 'text-[var(--hv-text-secondary)]',
              )}
            >
              <span
                class={cn(
                  'flex-none w-10 h-10 rounded-full grid place-items-center text-sm font-bold transition-colors border-2 border-transparent',
                  currentStep === s.n && 'bg-[var(--hv-app-accent-grad)] text-white shadow-[0_4px_12px_var(--hv-primary-light)]',
                  currentStep > s.n && 'bg-[var(--hv-success-bg)] text-[var(--hv-success)] border-[var(--hv-success)]',
                  currentStep < s.n && 'bg-[var(--hv-surface-container)] text-[var(--hv-text-tertiary)]',
                )}
              >
                {#if currentStep > s.n}<IconCheck size={15} />{:else}{s.n}{/if}
              </span>
              <span class="text-xs font-semibold whitespace-nowrap overflow-hidden text-ellipsis">{s.label}</span>
            </li>
          {/each}
        </ol>
        <div data-testid="raid-progress" class="h-1 w-full bg-[var(--hv-surface-container)] rounded-full overflow-hidden">
          <span class="block h-full bg-[var(--hv-primary)] rounded-full transition-[width] duration-300" style="width: {(currentStep / 3) * 100}%"></span>
        </div>
      </div>

      <!-- Step head -->
      <div class="px-5 pt-5 flex flex-col gap-1.5">
        <h2 class="text-xl font-bold tracking-tight text-[var(--hv-text-primary)] m-0">
          {currentStep === 1 ? $t('raid.manager.pick_style_title') : currentStep === 2 ? $t('raid.manager.bays_title') : $t('raid.manager.finalize_summary_title')}
        </h2>
        <p class="text-xs text-[var(--hv-text-secondary)] m-0 leading-relaxed">
          {currentStep === 1 ? $t('raid.manager.step1_subtitle') : currentStep === 2 ? $t('raid.manager.step2_subtitle') : $t('raid.manager.step3_subtitle')}
        </p>
      </div>

      <div class="px-5 py-4 flex flex-col gap-4">
        <!-- Step 1 — pick a level -->
        {#if currentStep === 1}
          <div data-testid="raid-type-grid" class="grid grid-cols-1 gap-3 @min-[540px]:grid-cols-3">
            {#each raidTypes as type}
              <button
                data-testid="raid-type-card"
                class:selected={selectedRaidType === type.id}
                class={cn(typeCard, selectedRaidType === type.id && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')}
                aria-pressed={selectedRaidType === type.id}
                onclick={() => selectRaidType(type.id)}
              >
                {#if type.recommended}
                  <span data-testid="raid-recommended" class="absolute -top-2.5 right-4 bg-[var(--hv-primary)] text-white text-micro font-bold uppercase tracking-[0.06em] px-2.5 py-1 rounded-full">
                    {$t('raid.manager.recommended')}
                  </span>
                {/if}
                <div class="flex items-end gap-1.5 h-14 mb-1" aria-hidden="true">
                  {#each diskColumns(type.id) as col}
                    <span class="flex flex-col gap-0.5 items-center">
                      {#each col.blocks as block}
                        <span
                          class={cn(
                            'w-5 h-[18px] rounded-[4px] flex-none',
                            block === 'data' && 'bg-[var(--hv-primary)]',
                            block === 'mirror' && 'bg-[var(--hv-primary-300)]',
                            block === 'parity' && 'bg-[var(--hv-warning)]',
                          )}
                        ></span>
                      {/each}
                      <span class="text-micro font-bold text-[var(--hv-text-tertiary)] mt-0.5 font-[family-name:var(--hv-font-mono)]">{col.label}</span>
                    </span>
                  {/each}
                </div>
                <h3 data-testid="raid-type-name" class="text-c-lg font-bold m-0">{type.name}</h3>
                <p class="text-c-xs text-[var(--hv-text-secondary)] m-0 leading-relaxed">{type.description}</p>
                <p class={cn('flex items-start gap-1.5 m-0 mt-auto text-c-xs font-semibold', type.protects ? 'text-[var(--hv-success-text)]' : 'text-[var(--hv-warning-text)]')}>
                  {#if type.protects}<IconShieldCheck size={16} class="flex-none mt-px" />{:else}<IconAlertTriangle size={16} class="flex-none mt-px" />{/if}
                  <span>{type.protection}</span>
                </p>
                <span class="text-micro tracking-[0.03em] text-[var(--hv-text-tertiary)]">{$t('raid.manager.min_disks_count', { count: String(type.minDisks) })}</span>
              </button>
            {/each}
          </div>
        {/if}

        <!-- Step 2 — pick disks -->
        {#if currentStep === 2}
          <div data-testid="raid-disk-head" class="flex items-center justify-between gap-2">
            <span class={tinyLabel}>{$t('raid.disks')}</span>
            <div class="flex items-center gap-2">
              <button
                class="inline-flex items-center gap-1.5 h-8 px-3 rounded-full border border-[var(--hv-border)] bg-[var(--hv-surface-container-lowest)] text-[var(--hv-text-secondary)] text-micro font-semibold cursor-pointer transition-colors hover:bg-[var(--hv-primary-light)] hover:text-[var(--hv-primary)] hover:border-[var(--hv-primary)]"
                onclick={selectAllDevices}
              >
                <IconCheck size={16} />
                <span>{$t('raid.manager.select_all')}</span>
              </button>
              <span class="text-xs font-bold px-2.5 py-1 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">
                {$t('raid.manager.disks_selected_count', { count: String(selectedDevices.length) })}
              </span>
            </div>
          </div>
          <div class="flex flex-col gap-2">
            {#each selectableDevices as device}
              <label
                data-testid="raid-disk-row"
                class:selected={selectedDevices.includes(device.name)}
                class={cn('selectable', diskRow, selectedDevices.includes(device.name) && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')}
              >
                <input
                  type="checkbox"
                  class="absolute inset-0 w-full h-full m-0 opacity-0 cursor-pointer appearance-none focus-visible:[&:focus-visible~.raid-check]:outline-2"
                  checked={selectedDevices.includes(device.name)}
                  onchange={() => toggleDevice(device.name)}
                />
                <span class={cn('raid-check flex-none w-6 h-6 rounded-md border-2 border-[var(--hv-border-hover)] grid place-items-center transition-colors', selectedDevices.includes(device.name) && 'bg-[var(--hv-primary)] border-[var(--hv-primary)] text-white')} aria-hidden="true">
                  <IconCheck size={16} />
                </span>
                <span class="flex-none w-[38px] h-[38px] rounded-lg bg-[var(--hv-surface-container)] text-[var(--hv-text-secondary)] grid place-items-center" aria-hidden="true"><IconServer size={20} /></span>
                <span class="flex-1 min-w-0 flex flex-col gap-px">
                  <span class="text-sm font-bold text-[var(--hv-text-primary)]">{device.name}</span>
                  <span class="flex items-center gap-2 min-w-0">
                    <span class="text-c-xs text-[var(--hv-text-tertiary)] whitespace-nowrap overflow-hidden text-ellipsis">{device.model || $t('storage.unknown')}</span>
                    <span class="inline-flex items-center gap-1 text-micro text-[var(--hv-success-text)] whitespace-nowrap">
                      <span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-success)] flex-none"></span>{$t('raid.manager.available')}
                    </span>
                  </span>
                </span>
                <span class="flex-none text-xs font-bold text-[var(--hv-primary)]">{formatBytes(device.size)}</span>
              </label>
            {/each}
            {#if selectableDevices.length === 0}
              <p data-testid="raid-empty-note" class="text-xs text-[var(--hv-text-tertiary)] m-0 py-4 text-center">{$t('raid.manager.no_disks_available')}</p>
            {/if}
          </div>

          {#if !canProceed && selectedDevices.length > 0 && selectedRaid}
            <p class="m-0 text-c-xs font-semibold text-[var(--hv-warning-text)]">{$t('raid.manager.select_more', { count: String(selectedRaid.minDisks - selectedDevices.length) })}</p>
          {/if}

          {#if estimatedCapacity > 0}
            {@const totalRaw = pickedSizes.reduce((a, b) => a + b, 0)}
            {@const usablePct = totalRaw > 0 ? Math.round((estimatedCapacity / totalRaw) * 100) : 0}
            {@const protectionPct = 100 - usablePct}
            <div class="bg-[var(--hv-primary-light)] rounded-xl p-4 flex flex-col gap-3">
              <div class="flex items-baseline justify-between gap-2">
                <span class={tinyLabel}>{$t('raid.manager.estimated_capacity')}</span>
                <span class="text-c-3xl font-bold text-[var(--hv-primary)] tracking-tight">{formatBytes(estimatedCapacity)}</span>
              </div>
              <div class="h-2 rounded-full bg-[var(--hv-surface-container)] overflow-hidden flex">
                <span class="block h-full bg-[var(--hv-primary)]" style="width: {usablePct}%"></span>
                <span class="block h-full bg-[var(--hv-warning)] opacity-50" style="width: {protectionPct}%"></span>
              </div>
              <div class="flex gap-4">
                <span class="flex items-center gap-1.5 text-micro text-[var(--hv-text-secondary)]"><span class="w-2 h-2 rounded-[2px] bg-[var(--hv-primary)] flex-none"></span>{$t('raid.manager.usable')}</span>
                <span class="flex items-center gap-1.5 text-micro text-[var(--hv-text-secondary)]"><span class="w-2 h-2 rounded-[2px] bg-[var(--hv-warning)] opacity-50 flex-none"></span>{$t('raid.manager.protection')}</span>
              </div>
            </div>
          {/if}
        {/if}

        <!-- Step 3 — name, filesystem, confirm -->
        {#if currentStep === 3}
          <div class="flex flex-col gap-2">
            <label class={tinyLabel} for="raidName">{$t('raid.name')}</label>
            <input id="raidName" class={raidInput} type="text" bind:value={raidName} placeholder={$t('raid.manager.name_placeholder')} autocomplete="off" spellcheck="false" />
          </div>

          <div class="flex flex-col gap-2">
            <span class={tinyLabel}>{$t('raid.filesystem')}</span>
            <div class="grid grid-cols-2 gap-2">
              {#each filesystems as fs}
                <button
                  data-testid="raid-fs-card"
                  class:selected={selectedFilesystem === fs.value}
                  class={cn(fsCard, selectedFilesystem === fs.value && 'border-[var(--hv-primary)] bg-[var(--hv-primary-light)]')}
                  aria-pressed={selectedFilesystem === fs.value}
                  onclick={() => (selectedFilesystem = fs.value)}
                >
                  <span class="text-xs font-bold text-[var(--hv-text-primary)]">{fs.label}</span>
                  <span class="text-micro text-[var(--hv-text-tertiary)] leading-relaxed">{fs.description}</span>
                </button>
              {/each}
            </div>
          </div>

          <dl class="m-0 bg-[var(--hv-surface-container-low)] rounded-xl p-4 grid grid-cols-2 gap-3">
            <div class="flex flex-col gap-0.5 min-w-0">
              <dt class={tinyLabel}>{$t('raid.manager.summary_type')}</dt>
              <dd class="m-0 text-xs font-bold text-[var(--hv-text-primary)]">{selectedRaid?.name}</dd>
            </div>
            <div class="flex flex-col gap-0.5 min-w-0">
              <dt class={tinyLabel}>{$t('raid.disks')}</dt>
              <dd class="m-0 text-xs font-bold text-[var(--hv-text-primary)]">{selectedDevices.length}</dd>
            </div>
            <div class="flex flex-col gap-0.5 min-w-0">
              <dt class={tinyLabel}>{$t('raid.manager.estimated_capacity')}</dt>
              <dd class="m-0 text-xs font-bold text-[var(--hv-text-primary)]">{formatBytes(estimatedCapacity)}</dd>
            </div>
            <div class="flex flex-col gap-0.5 min-w-0">
              <dt class={tinyLabel}>{$t('raid.manager.protection_label')}</dt>
              <dd class="m-0 text-xs font-bold text-[var(--hv-text-primary)]">{selectedRaid?.protection}</dd>
            </div>
          </dl>

          <div class="flex items-start gap-2.5 bg-[var(--hv-warning-bg)] text-[var(--hv-warning-text)] rounded-xl px-4 py-3 text-c-xs leading-relaxed">
            <IconAlertTriangle size={20} class="flex-none mt-px text-[var(--hv-warning)]" />
            <span>{$t('raid.warning.desc')}</span>
          </div>
        {/if}
      </div>

      <div class="flex items-center justify-between gap-3 px-5 py-4 border-t border-[var(--hv-border)] @min-[560px]:flex-row flex-col-reverse">
        {#if currentStep > 1}
          <Button variant="ghost" onclick={prevStep}><IconChevronLeft size={18} />{$t('raid.manager.back')}</Button>
        {:else}
          <span class="text-c-xs uppercase tracking-[0.05em] font-semibold text-[var(--hv-text-tertiary)]">{stepName}</span>
        {/if}
        {#if currentStep < 3}
          <Button variant="primary" disabled={!canProceed} onclick={nextStep}>
            {$t('raid.manager.continue_btn')}<IconChevronRight size={18} />
          </Button>
        {:else}
          <Button variant="primary" disabled={!raidName.trim() || creating} onclick={handleCreate}>
            {#if creating}<Spinner size="sm" />{:else}<IconCheck size={18} />{/if}
            {$t('raid.manager.create_array_button')}
          </Button>
        {/if}
      </div>
    </div>

    <p data-testid="raid-storage-note" class="text-c-xs text-center leading-relaxed text-[var(--hv-text-tertiary)] m-0">
      {$t('raid.manager.manage_in_storage')}
    </p>
  </div>
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>