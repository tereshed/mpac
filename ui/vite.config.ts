import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:4317',
        changeOrigin: true,
        configure(proxy) {
          // This fixed loopback development proxy is the only origin bridge.
          // The daemon retains strict Host/Origin checks and no browser CORS.
          proxy.on('proxyReq', request => {
            request.setHeader('Origin', 'http://127.0.0.1:4317')
          })
        },
      },
    },
  },
  build: { sourcemap: false },
})
