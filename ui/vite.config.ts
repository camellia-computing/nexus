import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  optimizeDeps: {
    include: [
      '@codemirror/autocomplete',
      'ajv/dist/2020.js',
    ],
  },
  server: {
    strictPort: true,
  },
});
