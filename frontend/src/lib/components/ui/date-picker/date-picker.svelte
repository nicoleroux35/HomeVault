<script lang="ts">
	/**
	 * HOMEVAULT DESIGN SYSTEM - DatePicker primitive
	 * Composed from the shadcn-svelte Popover + Calendar (bits-ui) plus date-fns
	 * formatting. Public `value` is a plain `Date | null` (matches the rest of
	 * the app's date-fns usage); the bits-ui Calendar works on
	 * `@internationalized/date` `DateValue`s, converted at the boundary.
	 */
	import { format } from "date-fns";
	import { CalendarDate, getLocalTimeZone, type DateValue } from "@internationalized/date";
	import CalendarIcon from "@lucide/svelte/icons/calendar";
	import { cn } from "$lib/utils.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Calendar } from "$lib/components/ui/calendar/index.js";
	import * as Popover from "$lib/components/ui/popover/index.js";

	let {
		value = $bindable(null),
		placeholder = "",
		locale = "fr-FR",
		dateFormat = "dd/MM/yyyy",
		disabled = false,
		id = "",
		class: className = "",
	}: {
		value?: Date | null;
		placeholder?: string;
		locale?: string;
		dateFormat?: string;
		disabled?: boolean;
		id?: string;
		class?: string;
	} = $props();

	let open = $state(false);

	function toCalendarDate(date: Date): DateValue {
		return new CalendarDate(date.getFullYear(), date.getMonth() + 1, date.getDate());
	}

	const calendarValue = $derived(value ? toCalendarDate(value) : undefined);

	function handleSelect(next: DateValue | undefined) {
		if (next) {
			value = next.toDate(getLocalTimeZone());
		} else {
			value = null;
		}
		open = false;
	}

	function clear() {
		value = null;
		open = false;
	}
</script>

<Popover.Root bind:open>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				{id}
				{disabled}
				variant="outline"
				class={cn(
					"w-full justify-start gap-2 text-left font-normal",
					!value && "text-muted-foreground",
					className
				)}
			>
				<CalendarIcon class="size-4 shrink-0" />
				<span class="flex-1 truncate">{value ? format(value, dateFormat) : placeholder}</span>
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content class="w-auto p-0" align="start">
		<Calendar
			type="single"
			value={calendarValue}
			onValueChange={(next) => handleSelect(next)}
			{locale}
		/>
		{#if value}
			<div class="flex justify-end p-2 pt-0">
				<Button variant="ghost" size="sm" onclick={clear}>
					Clear
				</Button>
			</div>
		{/if}
	</Popover.Content>
</Popover.Root>