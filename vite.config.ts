import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: '127.0.0.1',
    hmr: {
      protocol: 'ws',
      host: '127.0.0.1',
    },
    watch: {
      ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**', '**/l4a/**'],
    },
  },
  test: {
    environment: 'jsdom',
  },
})
