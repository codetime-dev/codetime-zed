# CodeTime for Zed

Automatic coding-time tracking for [codetime.dev](https://codetime.dev), ported
from [codetime-vscode](https://github.com/Data-Trekkers/codetime-vscode).

Open files in Zed and your activity (project, language, file, git branch/origin,
read vs. write) is reported to the CodeTime API. View the stats on your
dashboard.

## How it works (and what's different from VS Code)

Zed extensions cannot subscribe to editor events or draw a status-bar item the
way the VS Code API allows. The only hook into editing activity that Zed exposes
to extensions is the **language server protocol**. So this project ships two
parts:

| Part           | Crate         | Role                                                             |
| -------------- | ------------- | --------------------------------------------------------------- |
| Zed extension  | `codetime-zed` (WASM) | Registers and launches the language server for every language. |
| Language server | `codetime-ls` (native) | Receives `didOpen`/`didChange`/`didSave`, POSTs event logs to the CodeTime API. |

Consequence: there is **no in-editor status bar with your total time** — that VS
Code feature has no Zed equivalent. Tracking (the core feature) works the same.

## Configuration

If you have already signed in with another CodeTime client (the CLI, the VS Code
extension, …), a token is in `~/.codetime/config.json` and this extension picks
it up automatically — no setup needed.

To set it explicitly, get your token from [codetime.dev](https://codetime.dev)
and add it to Zed's `settings.json`:

```json
{
  "lsp": {
    "CodeTime": {
      "initialization_options": {
        "token": "<your-token>"
      }
    }
  }
}
```

The token is resolved in this order, first match wins:

1. `initialization_options.token` in `settings.json` (above)
2. the `CODETIME_TOKEN` environment variable in the shell Zed inherits
3. the `token` field of `~/.codetime/config.json`

Optional key under `initialization_options`: `api_url` overrides the API base
(default `https://api.codetime.dev`). An `HTTPS_PROXY` / `HTTP_PROXY` env var is
honored automatically.

On startup the language server logs which source the token came from, e.g.
`CodeTime: token loaded from ~/.codetime/config.json`, or a warning if none was
found. View it in Zed via the command palette → **dev: open language server
logs** → `CodeTime`.

## Installing as a dev extension

The published flow downloads a prebuilt `codetime-ls` from GitHub releases. To
run from source:

1. Build and expose the language server on your `PATH` (the extension prefers a
   `codetime-ls` found there before downloading one):

   ```sh
   cargo install --path codetime-ls
   # or: cargo build --release --manifest-path codetime-ls/Cargo.toml \
   #     && cp codetime-ls/target/release/codetime-ls ~/.local/bin/
   ```

2. In Zed, open the command palette → **zed: install dev extension** and select
   this repository's root. Zed compiles `src/lib.rs` to WASM and loads it.

3. Add your token (see above) and start editing. Watch the language server log
   (command palette → **dev: open language server logs** → `CodeTime`) to
   confirm events are sent.

## Building

```sh
cargo build --manifest-path codetime-ls/Cargo.toml   # native language server
rustup target add wasm32-wasip1                       # once
cargo build --target wasm32-wasip1                    # the extension itself
```

`codetime-ls` lives in its own Cargo workspace so the extension's
`wasm32-wasip1` build never tries to compile it (its tokio/reqwest
dependencies don't target wasm).
