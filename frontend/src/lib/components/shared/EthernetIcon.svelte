<script lang="ts">
  let { size = 20, strokeWidth = 1.91, ...rest }: { size?: number; strokeWidth?: number; [key: string]: any } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" {...rest}>
  <rect x="1.5" y="1.5" width="21" height="21" rx="1.91" fill="none" stroke="currentColor" stroke-miterlimit="10" stroke-width={strokeWidth}/>
  <polygon points="5.32 6.27 5.32 13.91 7.23 13.91 7.23 15.82 10.09 15.82 10.09 17.73 13.91 17.73 13.91 15.82 16.77 15.82 16.77 13.91 18.68 13.91 18.68 6.27 5.32 6.27" fill="none" stroke="currentColor" stroke-miterlimit="10" stroke-width={strokeWidth}/>
  <line x1="8.18" y1="9.14" x2="8.18" y2="6.27" fill="none" stroke="currentColor" stroke-miterlimit="10" stroke-width={strokeWidth}/>
  <line x1="12" y1="9.14" x2="12" y2="6.27" fill="none" stroke="currentColor" stroke-miterlimit="10" stroke-width={strokeWidth}/>
  <line x1="15.82" y1="9.14" x2="15.82" y2="6.27" fill="none" stroke="currentColor" stroke-miterlimit="10" stroke-width={strokeWidth}/>
</svg>
