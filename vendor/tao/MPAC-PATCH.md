# MPAC iOS cold URL delivery

Upstream: Tao 0.37.2 from crates.io; original LICENSE retained.

Only iOS URL delivery is changed. Legacy launch options are queued before app-ready; scene URL contexts are delivered after app-ready; cold scene user activities and warm handlers share safe parsing without raw URL logs. Registry sources are untouched. See MPAC-cold-links.patch for the complete source delta.

Reason: https://github.com/tauri-apps/tao/issues/1248

Remove the workspace patch after a pinned upstream release fixes both cold paths and passes MPAC cold/warm native tests. The scene userActivities path delivers HTTPS Universal Links at cold launch; domain verification and entitlements are configured by MPAC, outside Tao.
