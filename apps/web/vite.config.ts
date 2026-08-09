import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 7735,
    proxy: {
      '/v1': 'http://127.0.0.1:7734',
      '/healthz': 'http://127.0.0.1:7734',
    },
  },
})

