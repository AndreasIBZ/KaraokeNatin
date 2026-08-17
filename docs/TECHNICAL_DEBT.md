# FESTEJAR Technical Debt

## Legacy `karaokenatin` Identifiers

Some internal identifiers still use the old `karaokenatin` name and should not be renamed casually because they affect package resolution, persisted migrations, build scripts, Android namespaces, or compatibility with already-exported data.

Known legacy technical identifiers:

- npm workspace package names such as `@karaokenatin/shared` and `@karaokenatin/host`
- Rust/Android generated package paths under `apps/host/src-tauri/gen/android`
- legacy migration paths that read old `KaraokeNatin` data folders
- compatibility fields such as `karaokenatin` accepted when importing older playlist exports
- historical lower-case artifact names in build scripts

User-facing product text should use `FESTEJAR`.
