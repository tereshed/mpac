# MPAC UI

This React workspace is served by the Rust daemon. The built files in `dist/` and the API share an origin.

```sh
npm ci --ignore-scripts
npm run build
npm test
```

For development, `npm run dev` listens on `127.0.0.1` and proxies API calls to `127.0.0.1:4317`. The local Vite proxy rewrites the Host and Origin headers for the daemon. Production serves the interface directly from the daemon.

Sign in with the launch URL or enter the token manually. MPAC removes the token fragment from the address bar and verifies access to `/api/state` before saving the credential. The token is stored in local storage for the current origin and sent to the API in the Authorization header. Older session storage credentials migrate after validation. Sign-out clears both stores. If storage is unavailable, the current tab can keep the token in memory.

The terminal forwards PTY bytes to xterm and uses byte offsets to track the stream. Changing sessions starts replay from the beginning of the available stream. The interface indicates gaps in retained history. Process status describes the CLI process. Agent activity uses separate events when available.

The Markdown viewer treats documents as data. Relative links open files within the selected folder through the workspace API. External HTTP and HTTPS links open separately without a referrer.
