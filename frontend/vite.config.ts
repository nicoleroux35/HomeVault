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
        // `@lucide/svelte` carries "svelte" in its path, so a bare
        // `id.includes('svelte')` catch-all would swallow the whole icon pack
        // into `svelte-vendor`. Match the icon package first, then the
        // framework runtime itself.
        manualChunks: (id: string) => {
          if (!id.includes('node_modules')) return;
          if (id.includes('@lucide/svelte')) return 'icons-lucide';
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
      // Read `browserslist` from package.json (Safari/iOS >= 15, Firefox ESR)
      // instead of the implicit `defaults` set: the compatibility suite tests
      // WebKit, so the CSS targets must include it.
      targets: browserslistToTargets(browserslist(undefined, { path: import.meta.dirname })),
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
