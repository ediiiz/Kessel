/**
 * shadcn-svelte's support module, fetched from its registry (`registry:lib` item `utils`) and kept
 * verbatim apart from this comment and the formatting the repo's `oxfmt` applies.
 *
 * `cn` is the class merger every `src/lib/components/ui/**` component calls: `clsx` flattens the
 * conditional forms, then `tailwind-merge` resolves conflicts by *Tailwind semantics* rather than
 * source order, so a `class="p-6"` prop passed to a component whose base is `p-4` wins instead of
 * producing both. The `WithoutChild` / `WithElementRef` helpers are the prop-type surgery those
 * components use to re-expose `bits-ui`'s delegated-element API.
 *
 * Do not hand-edit: `shadcn-svelte add` overwrites this file. Kessel's own helpers belong elsewhere.
 */
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

export type WithoutChild<T> = T extends { child?: unknown } ? Omit<T, 'child'> : T;
export type WithoutChildren<T> = T extends { children?: unknown } ? Omit<T, 'children'> : T;
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & { ref?: U | null };
