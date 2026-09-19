import react from '@vitejs/plugin-react'
import stylex from '@stylexjs/unplugin'
import { defineConfig } from 'vite'

export default defineConfig(({ mode }) => ({
  plugins: [
    // Keep StyleX before the React plugin to preserve Fast Refresh.
    stylex.vite({
      useCSSLayers: true,
      dev: mode !== 'production',
      unstable_moduleResolution: { type: 'commonJS' },
    }),
    react(),
  ],
  server: {
    host: '127.0.0.1',
    port: 7735,
    proxy: {
      '/v1': 'http://127.0.0.1:7734',
      '/healthz': 'http://127.0.0.1:7734',
      // Daemon's utoipa Swagger UI — Index screen links here for API docs.
      '/docs': 'http://127.0.0.1:7734',
      '/openapi.json': 'http://127.0.0.1:7734',
    },
  },
}))
