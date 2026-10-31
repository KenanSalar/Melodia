---
paths:
  - crates/melodia-app/src/services/updater/**/*.rs
  - crates/melodia-platform/src/services/platform/install_kind/**/*.rs
  - crates/melodia/Cargo.toml
  - packaging/melodia-update-helper
  - packaging/com.github.kenansalar.melodia.update.policy
  - scripts/build-latest-json.py
  - scripts/build-rpm.sh
---

# In-app updater

- **The polkit helper's path is spelled in four places**: `swap.rs`'s `HELPER` and its `rpm`/`deb`
  argv, `packaging/melodia-update-helper`'s `case`, the policy's `exec.path`, and both packages'
  install lines (`build-rpm.sh`, `crates/melodia/Cargo.toml`'s deb assets). Nothing pins them
  together.
- **A platform key is spelled in three tables**: `updater_contract.rs` holds `target::format_key`
  to `build-latest-json.py`'s `PLATFORM_PATTERNS`, but nothing holds
  `staging::install_method_for_key` to either.
