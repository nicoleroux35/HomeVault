<!-- ========================================================================
     File          : Icon.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 10/11/2026 by Nicolas Le Roux
     Version       : 2.0.0

     Description:
       Dynamic icon by kebab-case name. Resolves through the Lucide registry
       used by the v2 design (outline, stroke 1.8); no sprite injection.

     Dependencies:
       - svelte (>= 5), $lib/icons/lucide-registry

     Usage:
       `<Icon name="folder" size={20} />`.
     ======================================================================== -->
<script lang="ts">
  import { getLucideIcon } from '$lib/icons/lucide-registry';

  export let name: string;
  export let size: number = 24;
  /** v2 icon weight. Callers may still override it per glyph. */
  export let strokeWidth: number = 1.8;
  export let stroke: number = strokeWidth;
  export let className: string = '';
  export let color: string = '';
  export let style: string = '';

  let _class = '';
  export { _class as class };

  $: effectiveStroke = stroke !== strokeWidth ? stroke : strokeWidth;
  $: IconComponent = getLucideIcon(name);
  $: svgClasses = [className, _class].filter(Boolean).join(' ');
</script>

<svelte:component
  this={IconComponent}
  size={size}
  strokeWidth={effectiveStroke}
  color={color || undefined}
  class={svgClasses}
  style={style || undefined}
  aria-hidden="true"
/>
