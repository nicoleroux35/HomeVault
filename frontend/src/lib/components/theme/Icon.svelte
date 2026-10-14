<!-- ========================================================================
     File          : Icon.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Unified icon component: resolves a name across the bundled icon
       libraries (Tabler/Phosphor, auto-detected or forced) and renders the
       matching SVG with controllable size, stroke and colour.

     Dependencies:
       - svelte (>= 5), $lib/icons/mapping-unified

     Usage:
       `<Icon name="tabler-folder" size={20} />`.
     ======================================================================== -->
<script lang="ts">
  import { getIcon, hasIcon, iconLibraries, type IconLibrary } from '$lib/icons/mapping-unified';

  export let name: string;
  export let size: number = 24;
  export let strokeWidth: number = 2;
  export let stroke: number = strokeWidth;
  export let className: string = '';
  export let library: IconLibrary | 'auto' = 'auto';
  export let color: string = '';
  export let style: string = '';

  let _class = '';
  export { _class as class };

  // Accept both stroke and strokeWidth, prefer stroke if explicitly set
  $: effectiveStroke = stroke !== strokeWidth ? stroke : strokeWidth;

  // Get the icon ID based on library preference
  $: iconId = (() => {
    // If name already has a prefix, use it directly
    if (name.startsWith('tabler-') || name.startsWith('phosphor-')) {
      return name;
    }

    // Use library preference or auto-detect
    if (library !== 'auto' && library) {
      return getIcon(name, library) || name;
    }

    // Auto: get first available
    return getIcon(name) || name;
  })();

  // Check if the icon exists
  $: exists = hasIcon(name.replace(/^(phosphor|tabler)-/, '')) || iconId.includes('-');

  // Phosphor symbols in the shared sprite are solid filled glyphs, unlike
  // tabler's stroke-based outlines - forcing fill:none/stroke:currentColor on
  // them (the tabler convention) renders them as broken wireframes.
  $: isFilledGlyph = iconId.startsWith('phosphor-');

  $: svgClasses = [className, _class].filter(Boolean).join(' ');
  $: svgStyle = [style, color ? `color: ${color}` : ''].filter(Boolean).join('; ');
</script>

<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={isFilledGlyph ? 'currentColor' : 'none'}
  stroke={isFilledGlyph ? 'none' : 'currentColor'}
  stroke-width={isFilledGlyph ? undefined : effectiveStroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  class={svgClasses}
  style={svgStyle}
  aria-hidden="true"
>
  <use href={`#${iconId}`} />
</svg>
