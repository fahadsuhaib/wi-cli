# Changelog

## 1.0.0

- Remote boxes over SSH moved to wibox (wibox.dev); wi is now local-only.
- Removed: `wi <box>` and `wi <ssh-alias>`, the `box` entries of the config, `wi fetch`, and the menu entries
  Reconnect, Download, Detach (keep sessions) and End sessions and quit. `wi` takes a local folder (or none).
- Removed the `download_dir` and `download_confirm_mb` settings (they only applied to remote files).
- Everything else is unchanged: tabs, previews, diffs, the explorer, search, themes, dictation, update and
  restart, reopening tabs. Saved state from 0.9 still loads.
- Smaller binary and fewer dependencies (no SSH or SFTP crates).

Earlier releases: see the GitHub releases page.
