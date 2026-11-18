# Progress log

## 2026-11-18

- Explored vms.rs (registry, Selector, list), run.rs (registration), dev/{list,cli,plan,boot}.rs,
  vk-hub ui/{dev,actions,local}.rs. Wrote SPEC.md.
- `./dev.sh` hung: it loops on `mkdir -p` of its VM state dir under the read-only
  `~/.local/state`. Switched to the host toolchain (1.99.1) offline, gnu target, own target dir;
  fetched the crates the lockfile wanted into the repo's cargo home.
- Steps 1–5 implemented and committed together (they share main.rs and vms.rs). Old `vk list`
  table/JSON view code and its tests removed; `--field` tests now run on literal JSON.
- Legacy dev dir names must not follow symlinks (`named_stop_needs_neither…` caught it).
- Steps 6–7, then 8 (hub lockstep), then 9 (docs) committed.
- Consolidation: clippy found an unused `published_cell` and `Index::remove`; the latter now
  serves `vk rm` of a name whose dir is gone. Two full runs: only the 12 baseline failures.
- Smoke test with the debug binary and XDG dirs under /tmp: all verbs behave on fake state;
  `vk run`/`vk start` reach the boot and stop at the missing `/dev/kvm`.
