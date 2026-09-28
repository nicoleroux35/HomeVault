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
        manualChunks: (id: string) => {
          if (id.includes('node_modules')) {
            if (id.includes('svelte') || id.includes('svelte/')) {
              return 'svelte-vendor';
            }
            if (id.includes('lucide')) {
              return 'lucide';
            }
          }
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
