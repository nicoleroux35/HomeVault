<script lang="ts">
  import { cn } from '$lib/utils.js';

  interface Props {
    /** Checked state (bindable). */
    checked?: boolean;
    /** Disable interaction. */
    disabled?: boolean;
    /** Optional inline text rendered next to the track. Most callers leave
     *  this empty and put the wording in the surrounding row instead. */
    label?: string;
    /** Accessible name. Required when there is no `label` and no
     *  `aria-labelledby` on the row — a bare track has no text content. */
    ariaLabel?: string;
    /** Id of an external element that names this switch. Use it when the
     *  visible label lives in the surrounding row (a `<span id=…>` next to a
     *  hint line): the switch then carries no inline text and the accessible
     *  name is the row's own wording, with no duplicated string. */
    ariaLabelledby?: string;
    /** Fill colour of the track when checked. Any CSS colour; pass a
     *  `--hv-*` token so the control stays on the palette. Defaults to
     *  `--hv-primary`. */
    accent?: string;
    /** Extra classes merged onto the root label. */
    class?: string;
    /** Test hook rendered on the root label. */
    'data-testid'?: string;
    /** Notified with the new state on every user toggle. Use this (with a
     *  one-way `checked`) when the source of truth is a store field that
     *  cannot be bound directly — e.g. `$monitorStore[def.visibleKey]`. */
    onchange?: (checked: boolean) => void;
  }

  let {
    checked = $bindable(false),
    disabled = false,
    label,
    ariaLabel,
    ariaLabelledby,
    accent,
    class: klass = '',
    'data-testid': testId,
    onchange,
  }: Props = $props();

  /* The accent travels to the stylesheet as a custom property so the
     `:checked` rule stays in CSS instead of an inline style per element. */
  const accentStyle = $derived(accent ? `--hv-switch-accent: ${accent}` : '');
</script>

<!--
  Switch — the Settings screen's toggle control.

  Deliberately a native `<input type="checkbox">` (visually hidden) inside a
  `<label>`, NOT a `role="switch"` button: the E2E suite drives these through
  `.settings-toggle-label` + `input[type="checkbox"]` (`flipSwitch()`), and a
  native input keeps label-click activation, `isChecked()` and keyboard
  toggling working for free. The visible 40x22 track is a styled sibling, so
  it can be painted from `:checked` without any JS state mirror.

  For the button-styled switch (shadcn/bits-ui, `role="switch"`), use
  `theme/Toggle` instead.
-->
<label
  class={cn('settings-toggle-label', klass)}
  style={accentStyle}
  data-testid={testId}
>
  <input
    type="checkbox"
    class="hv-switch-input"
    bind:checked
    {disabled}
    aria-label={ariaLabel}
    aria-labelledby={ariaLabelledby}
    onchange={(e) => onchange?.((e.currentTarget as HTMLInputElement).checked)}
  />
  <span class="hv-switch-track" aria-hidden="true">
    <span class="hv-switch-knob"></span>
  </span>
  {#if label}
    <span class="hv-switch-text">{label}</span>
  {/if}
</label>

<style>
  .settings-toggle-label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    flex: none;
    cursor: pointer;
  }

  .settings-toggle-label:has(.hv-switch-input:disabled) {
    cursor: not-allowed;
  }

  /* Visually hidden but still focusable and clickable — the label is the hit
     target, the input is the state. Same box as Tailwind's `sr-only`. */
  .hv-switch-input {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .hv-switch-track {
    position: relative;
    display: inline-block;
    flex: none;
    width: 40px;
    height: 22px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-settings-toggle-off);
    transition: background var(--hv-chrome-dur-fast, 150ms) ease;
  }

  /* Touch hit area: a transparent 44px square centred on the track, so the
     switch meets --hv-touch-target-min (WCAG 2.5.5 / Apple / Material)
     without growing the visible 40x22 geometry. Any click on it lands on the
     label, which toggles the checkbox natively. */
  .hv-switch-track::before {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: var(--hv-touch-target-min, 44px);
    height: var(--hv-touch-target-min, 44px);
    transform: translate(-50%, -50%);
  }

  .hv-switch-knob {
    position: absolute;
    top: 3px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-on-primary);
    box-shadow: var(--hv-settings-knob-shadow);
    transition: transform var(--hv-chrome-dur-fast, 150ms) ease;
  }

  /* 40px track - 16px knob - 2px inset on both sides = 20px of travel. */
  .hv-switch-input:checked + .hv-switch-track {
    background: var(--hv-switch-accent, var(--hv-primary));
  }

  .hv-switch-input:checked + .hv-switch-track .hv-switch-knob {
    transform: translateX(20px);
  }

  .hv-switch-input:focus-visible + .hv-switch-track {
    box-shadow: var(--hv-focus-ring);
  }

  .hv-switch-input:disabled + .hv-switch-track {
    opacity: 0.5;
  }

  .hv-switch-text {
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-title);
  }
</style>
