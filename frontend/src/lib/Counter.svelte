<!-- ========================================================================
     File          : Counter.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Svelte template demo counter left over from project scaffolding —
     unused by the application (candidate for deletion).

     Dependencies:
       - svelte (>= 5)

     Usage:
       None — scaffolding leftover.
     ======================================================================== -->
<script lang="ts">
  let count: number = $state(0)
  const increment = () => {
    count += 1
  }
</script>

<button type="button" class="counter" onclick={increment}>
  Count is {count}
</button>
