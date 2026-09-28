<script lang="ts">
	import { Pagination as PaginationPrimitive } from "bits-ui";
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import { cn } from "$lib/utils.js";
	import { buttonVariants } from "../button/index.js";
	import { t } from "$lib/stores/i18n";

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: PaginationPrimitive.NextButtonProps = $props();
</script>

{#snippet Fallback()}
	<span>{$t('common.next')}</span>
	<ChevronRightIcon class={cn("cn-rtl-flip size-4", className)} />
{/snippet}

<PaginationPrimitive.NextButton
	bind:ref
	aria-label={$t('common.next_page')}
	class={cn(buttonVariants({ variant: "ghost" }), "pr-2!", className)}
	{...restProps}
>
	{#if children}
		{@render children?.()}
	{:else}
		{@render Fallback()}
	{/if}
</PaginationPrimitive.NextButton>
