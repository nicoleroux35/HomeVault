import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import browserslist from 'browserslist';
import { browserslistToTargets } from 'lightningcss';
import path from 'path';

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      $lib: path.resolve(import.meta.dirname, './src/lib'),
    },
  },
  build: {
    outDir: '../static/dist',
    emptyOutDir: true,
    cssMinify: 'lightningcss',
    rollupOptions: {
      output: {
        // Order matters: `@lucide/svelte` and `@tabler/icons-svelte` both
        // carry "svelte" in their path, so a bare `id.includes('svelte')`
        // catch-all swallows the whole icon packs — that is what pushed the
        // old `svelte-vendor` chunk past Vite's 500 kB warning. Match the icon
        // packs first, then the framework runtime itself.
        manualChunks: (id: string) => {
          if (!id.includes('node_modules')) return;
          if (id.includes('@lucide/svelte')) return 'icons-lucide';
          if (id.includes('@tabler/icons-svelte')) return 'icons-tabler';
          if (id.includes('/svelte/') || id.includes('@sveltejs/')) {
            return 'svelte-vendor';
          }
          if (id.includes('svelte-sonner')) return 'svelte-vendor';
        },
      },
    },
  },
  css: {
    lightningcss: {
      targets: browserslistToTargets(browserslist('defaults')),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
      },
    },
  },
})
