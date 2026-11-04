# deepmerge (local hardened fork)

`@sveltejs/vite-plugin-svelte` depends on `deepmerge`, and **no upstream release
fixes** GHSA-gcqx-w48m-w2h4 / CVE-2026-93753 (prototype poisoning through
`__proto__`, `constructor` and `prototype` keys; affected `>= 4.2.0, <= 4.3.1`).

This directory is a drop-in replacement with the same public API
(`deepmerge(target, source, options)`, `arrayMerge`, `customMerge`, `clone`,
`isMergeableObject`) that refuses to merge those three keys and reports
**4.3.2**, so dependency tooling sees a version outside the vulnerable range.
The upstream MIT licence applies.

`frontend/package.json` installs it through an npm override:

```json
"overrides": { "deepmerge": "file:./vendor/deepmerge" }
```

Run its tests with `node vendor/deepmerge/test.mjs` from `frontend/`.

**Remove it** once `@sveltejs/vite-plugin-svelte` (or another upstream) ships a
`deepmerge` version above 4.3.1: delete the override and this directory, then
`npm install`.
