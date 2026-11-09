# MPAC · Multi Project Agent Control

[User guide with screenshots](docs/user-guide.md)

MPAC is a local workspace for terminal agents. Group repositories into projects,
start CLI sessions, read replies and files, and review changes across project roots.
The Rust service runs agents on your computer. The React workspace is available in
a browser, a native macOS client, and companion apps for iPhone, iPad, and Android.

- Run Claude Code, Codex, Hermes, a shell, or a custom command in a PTY.
- See which supported agents are working, waiting for input, or have a new result.
  Open the latest captured Claude Code or Codex reply even after terminal scrollback is gone.
- Read Markdown, inspect file diffs and Git branches, and send review notes to an agent.
- Set launch defaults, choose an environment file per session, and use optional voice input.
- Connect a phone over HTTPS on your local network or a private Tailscale network.
  Approve each device on the host. Desktop MPAC prevents automatic idle sleep while it is running.

## Run locally

Install Node.js 22+ and the Rust toolchain pinned in `rust-toolchain.toml`. Run this
from the repository root:

```sh
./scripts/start-local.sh
```

Open the local URL printed by MPAC. The URL contains a workspace token, so keep it
private. Try the demo agent to check the interface without calling a model. For
real agents, install and authenticate the respective CLI separately.

On macOS, build a local native app with `./scripts/build-desktop.sh`. The signed
bundle is created under `target/release/bundle/macos/MPAC.app`. To run the Rust
host on Linux x86-64, build the host binaries locally with
`sh scripts/build-linux-host.sh` and Docker. Serve the built UI alongside the daemon.

## Mobile development

Set up the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for
your target, including Xcode for iOS or the Android SDK, NDK, and Java for Android.
Install the pinned Tauri CLI and initialize the native project from `mobile/`:

```sh
npm --prefix desktop ci
./scripts/bootstrap-rust.sh
export CARGO_HOME="$PWD/.tools/cargo"
export RUSTUP_HOME="$PWD/.tools/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-linux-android
(cd mobile && ../desktop/node_modules/.bin/tauri ios init --ci --skip-targets-install)
(cd mobile && ../desktop/node_modules/.bin/tauri android init --ci --skip-targets-install)
```

Run `sh scripts/build-mobile.sh ios` for the Apple Silicon simulator or
`sh scripts/build-mobile.sh android` for an Android debug APK. Initialize only
the platform you need. For iOS device distribution, configure your own Apple
team and app identity, set `APPLE_DEVELOPMENT_TEAM`, and use
`./scripts/build-testflight.sh BUILD_NUMBER`. A self-hosted Universal Links
deployment also needs your app identity and the corresponding association files
on your HTTPS domain.

In MPAC Desktop, open **Settings & connections → Phone & Telegram**, select a
local or Tailscale address, then create a pairing link. Open it in the phone app
and verify the matching code on the Mac. Sessions and project files stay on the host.

## Workspace data

MPAC keeps its local workspace, token, and runtime state in a private data directory.
Mobile pairing grants access to the workspace's terminal and project files. Connect
only devices you trust and use a private network for remote access. Keep workspace
data, terminal captures, credentials, and signed builds out of source control.

See [session environments](docs/session-environment.md) and
[terminal streaming](docs/terminal-streaming.md) for implementation details.
The vendored Tao patch and its upstream license are in [vendor/tao](vendor/tao).

## License

MPAC is available under the [MIT License](LICENSE). The vendored Tao code
retains its [Apache 2.0 license](vendor/tao/LICENSE).
