<script lang="ts">
  /**
   * HOMEVAULT DESIGN SYSTEM — DataTable (page composite, `app/` layer)
   *
   * The one standard data table (the pattern Docker / Users / Groups /
   * Shares / Journal used to re-implement per app): compact appliance chrome —
   * muted 13px headers, 44px rows, hairline separators, hover fill.
   *
   * Behaviour (W11 + W12, 2026-09-24):
   * - sticky header and frozen first column inside the table's own scroll
   *   area (`max-h-[75vh]`) — long lists never lose their headings;
   * - sortable columns rendered as real `<button>`s with `aria-sort` on the
   *   `<th>`: every column with a non-empty label sorts unless
   *   `sortable: false`; starts unsorted so the caller's own row order
   *   (e.g. Quotas users-before-groups) is the initial state;
   * - numeric columns (raw `number` values) right-align by default so
   *   tabular figures line up; explicit `align` always wins;
   * - `loading` renders `loadingRows` shimmer skeleton rows instead of a
   *   spinner (W11), announced once through a visually-hidden status cell;
   * - pagination (100 rows/page) engages only past 300 rows and stays
   *   dormant for the smaller appliance lists.
   *
   * The sticky header and frozen first column paint with `bg-card`: place
   * the table on a card surface (the standard window-body background).
   *
   * Usage:
   *   <DataTable columns={[{id:'name', label: t('x.name')}]} rows={items} rowKey={(r)=>r.id}>
   *     {#snippet cell({ row, column })}
   *       {column.id === 'name' ? <strong>{row.name}</strong> : row[column.id]}
   *     {/snippet}
   *   </DataTable>
   *
   * Without a `cell` snippet the raw `row[column.id]` value is rendered.
   */
  import type { Snippet } from 'svelte';
  import { cn } from '$lib/utils.js';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import { t } from '$lib/stores/i18n';
  import Icon from '$lib/components/theme/Icon.svelte';

  interface Column {
    id: string;
    label: string;
    /** Header/column alignment. Numeric columns right-align by default. */
    align?: 'left' | 'right' | 'center';
    /** Extra class on every cell of this column. */
    class?: string;
    /** Set false to opt out of sorting (empty labels never sort). */
    sortable?: boolean;
  }

  interface CellArgs {
    row: any;
    column: Column;
  }

  interface Props {
    columns: Column[];
    rows: any[];
    rowKey: (row: any) => string;
    /** Custom cell renderer. Falls back to `row[column.id]`. */
    cell?: Snippet<[CellArgs]>;
    /** Empty-state content; default: a muted "no rows" line. */
    empty?: Snippet;
    emptyText?: string;
    /** Show skeleton rows instead of the body. */
    loading?: boolean;
    /** Number of shimmer rows drawn while `loading`. */
    loadingRows?: number;
    /** Row click handler (rows are not focusable unless this is set). */
    onRowClick?: (row: any) => void;
    /** Accessible caption for the table. */
    label?: string;
    /** Extra class on the wrapping element. */
    class?: string;
  }

  let {
    columns,
    rows,
    rowKey,
    cell,
    empty,
    emptyText,
    loading = false,
    loadingRows = 5,
    onRowClick,
    label,
    class: extraClass = '',
  }: Props = $props();

  const alignClass: Record<NonNullable<Column['align']>, string> = {
    left: 'text-left',
    right: 'text-right',
    center: 'text-center',
  };

  /** Dormant until the list outgrows three pages of 100 rows. */
  const PAGE_SIZE = 100;
  const PAGINATE_ABOVE = 300;

  let sortId = $state<string | null>(null);
  let sortDir = $state<'asc' | 'desc'>('asc');
  let page = $state(0);

  function isSortable(col: Column): boolean {
    return col.sortable !== false && col.label !== '';
  }

  function toggleSort(id: string): void {
    if (sortId === id) {
      sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    } else {
      sortId = id;
      sortDir = 'asc';
    }
    page = 0;
  }

  function compareValues(a: any, b: any): number {
    if (a == null && b == null) return 0;
    if (a == null) return 1;
    if (b == null) return -1;
    if (typeof a === 'number' && typeof b === 'number') return a - b;
    return String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: 'base' });
  }

  /** Explicit `align` wins; a raw number column right-aligns (tabular figures). */
  function autoAlign(col: Column): NonNullable<Column['align']> {
    if (col.align) return col.align;
    const sample = rows.find((r) => r != null && r[col.id] != null);
    if (sample && typeof sample[col.id] === 'number') return 'right';
    return 'left';
  }
  const colAligns = $derived(columns.map((c) => autoAlign(c)));

  const sortedRows = $derived.by(() => {
    if (sortId === null) return rows;
    const id = sortId;
    const dir = sortDir === 'asc' ? 1 : -1;
    return [...rows].sort((a, b) => dir * compareValues(a?.[id], b?.[id]));
  });

  const pageCount = $derived(
    rows.length > PAGINATE_ABOVE ? Math.max(1, Math.ceil(sortedRows.length / PAGE_SIZE)) : 1,
  );
  const safePage = $derived(Math.min(page, pageCount - 1));
  const visibleRows = $derived(
    pageCount > 1 ? sortedRows.slice(safePage * PAGE_SIZE, (safePage + 1) * PAGE_SIZE) : sortedRows,
  );
</script>

<div class={cn('hv-datatable w-full overflow-hidden', extraClass)}>
  {#if loading}
    <span class="sr-only" role="status">{$t('common.loading')}</span>
  {/if}
  <div class="w-full overflow-auto max-h-[75vh]">
    <table class="w-full border-collapse text-c-base">
      {#if label}
        <caption class="sr-only">{label}</caption>
      {/if}
      <thead>
        <tr>
          {#each columns as col, ci}
            <th
              scope="col"
              class={cn(
                'bg-card text-c-sm font-semibold whitespace-nowrap align-middle shadow-[inset_0_-1px_0_var(--hv-shell-border)]',
                isSortable(col) ? 'p-0' : 'px-3 py-2.5',
                ci === 0 ? 'sticky left-0 top-0 z-20' : 'sticky top-0 z-10',
                alignClass[colAligns[ci]],
                col.class,
              )}
              aria-sort={isSortable(col)
                ? sortId === col.id
                  ? sortDir === 'asc'
                    ? 'ascending'
                    : 'descending'
                  : 'none'
                : undefined}
            >
              {#if isSortable(col)}
                <button
                  type="button"
                  class={cn(
                    'flex w-full min-h-[var(--hv-touch-target-min)] items-center gap-1 px-3 text-c-sm font-semibold text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)]',
                    colAligns[ci] === 'right'
                      ? 'justify-end'
                      : colAligns[ci] === 'center'
                        ? 'justify-center'
                        : 'justify-start',
                    sortId === col.id && 'text-foreground',
                  )}
                  onclick={() => toggleSort(col.id)}
                >
                  {col.label}
                  {#if sortId === col.id}
                    <Icon
                      name={sortDir === 'asc' ? 'caret-up' : 'caret-down'}
                      size={14}
                      class="text-[var(--hv-primary)]"
                    />
                  {:else}
                    <span class="flex flex-col -space-y-1 opacity-45" aria-hidden="true">
                      <Icon name="caret-up" size={9} />
                      <Icon name="caret-down" size={9} />
                    </span>
                  {/if}
                </button>
              {:else}
                {col.label}
              {/if}
            </th>
          {/each}
        </tr>
      </thead>
      <tbody aria-busy={loading}>
        {#if loading}
          {#each Array(loadingRows) as _, i}
            <tr class="border-b border-[var(--hv-shell-border-soft)]" aria-hidden="true">
              {#each columns as col, j}
                <td
                  class={cn(
                    'px-3 py-3 align-middle',
                    j === 0 && 'sticky left-0 z-[5] bg-card',
                    alignClass[colAligns[j]],
                    col.class,
                  )}
                >
                  <span class="hv-skeleton-text block" style="width: {45 + ((i * 31 + j * 17) % 36)}%"></span>
                </td>
              {/each}
            </tr>
          {/each}
        {:else if rows.length === 0}
          <tr>
            <td colspan={columns.length} class="py-10 text-center text-muted-foreground">
              {#if empty}
                {@render empty()}
              {:else}
                <span class="text-c-base">{emptyText ?? ''}</span>
              {/if}
            </td>
          </tr>
        {:else}
          {#each visibleRows as row (rowKey(row))}
            <tr
              class={cn(
                'group border-b border-[var(--hv-shell-border-soft)] transition-colors',
                onRowClick ? 'cursor-pointer hover:bg-[var(--hv-subtle-hover)]' : '',
              )}
              onclick={onRowClick ? () => onRowClick(row) : undefined}
              tabindex={onRowClick ? 0 : undefined}
              onkeydown={(e) => {
                const click = onRowClick;
                if (click) activateOnEnterOrSpace(e, () => click(row));
              }}
            >
              {#each columns as col, ci}
                <td
                  class={cn(
                    'px-3 py-2.5 align-middle',
                    ci === 0 && 'sticky left-0 z-[5] bg-card',
                    onRowClick && 'group-hover:bg-[var(--hv-subtle-hover)]',
                    alignClass[colAligns[ci]],
                    col.class,
                  )}
                >
                  {#if cell}
                    {@render cell({ row, column: col })}
                  {:else}
                    {row[col.id]}
                  {/if}
                </td>
              {/each}
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
  {#if pageCount > 1}
    <nav
      class="flex items-center justify-between gap-3 border-t border-[var(--hv-shell-border)] bg-card px-3 py-2"
      aria-label={$t('common.pagination_nav')}
    >
      <span class="text-c-sm text-muted-foreground" aria-live="polite"
        >{safePage * PAGE_SIZE + 1}–{Math.min((safePage + 1) * PAGE_SIZE, sortedRows.length)} / {sortedRows.length}</span
      >
      <span class="flex items-center gap-2">
        <button
          type="button"
          class="min-h-[var(--hv-touch-target-min)] cursor-pointer rounded-lg border border-[var(--hv-shell-border)] bg-card px-4 text-c-sm font-medium text-foreground transition-colors hover:bg-[var(--hv-shell-fill)] focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:cursor-not-allowed disabled:opacity-40"
          disabled={safePage <= 0}
          onclick={() => {
            page = Math.max(0, safePage - 1);
          }}
        >
          {$t('common.previous')}
        </button>
        <button
          type="button"
          class="min-h-[var(--hv-touch-target-min)] cursor-pointer rounded-lg border border-[var(--hv-shell-border)] bg-card px-4 text-c-sm font-medium text-foreground transition-colors hover:bg-[var(--hv-shell-fill)] focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:cursor-not-allowed disabled:opacity-40"
          disabled={safePage >= pageCount - 1}
          onclick={() => {
            page = safePage + 1;
          }}
        >
          {$t('common.next')}
        </button>
      </span>
    </nav>
  {/if}
</div>
