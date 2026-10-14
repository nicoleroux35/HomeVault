/* =============================================================================
 * File          : utils.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Tiny shared helpers for components: `cn` merges Tailwind class lists
 *   (conditional classes + conflict resolution), and the Without* / With-
 *   ElementRef types trim or extend component prop types where children
 *   would be re-declared.
 *
 * Dependencies:
 *   - clsx
 *   - tailwind-merge
 *
 * Usage:
 *   `import { cn } from '$lib/utils'` inside components.
 * =============================================================================
 */

import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChild<T> = T extends { child?: any } ? Omit<T, "child"> : T;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChildren<T> = T extends { children?: any } ? Omit<T, "children"> : T;
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & {
	ref?: U | null;
};