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

Get your token from [codetime.dev](https://codetime.dev), then add it to Zed's
`settings.json`:

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

Optional keys under `initialization_options`:

- `api_url` — override the API base (default `https://api.codetime.dev`).

Alternatively, set the `CODETIME_TOKEN` environment variable in the shell Zed
inherits. An `HTTPS_PROXY` / `HTTP_PROXY` env var is honored automatically.

## Installing as a dev extension

The published flow downloads a prebuilt `codetime-ls` from GitHub releases. To
run from source:

1. Build and expose the language server on your `PATH` (the extension prefers a
   `codetime-ls` found there before downloading one):

   ```sh
   cargo install --path codetime-ls
   # or: cargo build -p codetime-ls --release && cp target/release/codetime-ls ~/.local/bin/
   ```

2. In Zed, open the command palette → **zed: install dev extension** and select
   this repository's root. Zed compiles `src/lib.rs` to WASM and loads it.

3. Add your token (see above) and start editing. Watch the language server log
   (command palette → **dev: open language server logs** → `CodeTime`) to
   confirm events are sent.

## Building

```sh
cargo build -p codetime-ls                          # native language server
rustup target add wasm32-wasip1                      # once
cargo build -p codetime-zed --target wasm32-wasip1   # the extension itself
```
