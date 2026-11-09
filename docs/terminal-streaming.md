# Terminal streaming

MPAC streams terminal output over a persistent WebSocket. The daemon notifies subscribers when it receives new PTY bytes. The client batches short bursts of input over 8 milliseconds and sends them on the open connection. The CLI provides the terminal echo.

In the browser, the WebSocket connects to the daemon. On macOS, a Rust bridge connects the WebView to the verified daemon process. Rust retains the persistent daemon credential and gives the interface a temporary window credential. The bridge verifies a new socket before sending credentials, including when reconnecting.

For older daemons, the macOS bridge keeps the client-side WebSocket and fetches HTTP output from the daemon. It polls at 16 milliseconds during activity, with a delay of up to 120 milliseconds when idle, and wakes immediately after input. Older browser backends also use adaptive polling. The new streaming protocol takes effect after the daemon is updated and restarted.

## Ordering and recovery

The daemon sends output in chunks of up to 32 KiB. It waits for xterm to acknowledge rendering before sending the next chunk. On reconnect, the client waits for queued rendering to finish and resumes from the last rendered byte.

Input and resize commands are acknowledged independently. If an input acknowledgment is missing, check the terminal before retrying. The command may have reached the CLI already. The input queue is limited to 64 KiB including the frame being sent. Unicode, bracketed paste, and Shift+Enter preserve byte order. Closing the client or disconnecting leaves the CLI running.

The daemon keeps a 512 KiB tail of raw output. The toolbar shows a warning if some older output can no longer be replayed. New output does not scroll the reader away from older lines. Resize requests are coalesced into a rendering frame, with a fresh measurement after a dialog closes.

## Passive mobile replay geometry

HTTP output chunks and WebSocket frames include `geometry: {cols, rows}` at the start of the chunk and ordered `resizes: [{offset, cols, rows}]` at raw byte offsets. Zero-byte frames communicate idle resizes. Replay applies each resize between byte segments and preserves the UTF-8 decoder. The 512 KiB output limit also bounds the retained resize metadata. A passive mobile viewer uses the host terminal geometry and suppresses automatic terminal query replies during replay.

The mobile **Text** view wraps retained scrollback for the normal buffer. When a CLI switches to the alternate buffer, **Text** follows the current interactive screen. **History** opens a snapshot of retained parsed lines.

**Latest reply** reads the newest message supplied by the root Claude Code or Codex `Stop` hook. It stays available after reconnecting or restarting the daemon, even if the terminal lines have been dropped. MPAC stores one complete message per session, up to 192 KiB within the hook event limit, in the local workspace database. Access requires the authenticated session endpoint. Older transcripts, prompts, and tool outputs are outside this reply view. A Stop hook can be followed by a continuation. Reading the reply leaves the session state and read marker alone.
