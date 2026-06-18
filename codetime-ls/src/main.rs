//! CodeTime language server.
//!
//! Zed extensions cannot subscribe to editor events directly, so we register as
//! a language server and translate LSP `textDocument` notifications into
//! CodeTime event logs, mirroring the codetime-vscode extension.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        InitializeParams, InitializeResult, InitializedParams, MessageType, SaveOptions,
        ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
        TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    },
    Client, LanguageServer, LspService, Server,
};
use url::Url;

const DEFAULT_API_URL: &str = "https://api.codetime.dev";
const EVENT_PATH: &str = "/v3/users/event-log";
/// Read (non-write) events on the same file within this window are dropped to
/// avoid flooding the API while still capturing continuous activity.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(120);

// Event names match codetime-vscode's `events.ts`.
const EVENT_ACTIVATE_FILE_CHANGED: &str = "activateFileChanged";
const EVENT_FILE_EDITED: &str = "fileEdited";
const EVENT_FILE_SAVED: &str = "fileSaved";

#[derive(Parser)]
#[command(version, about = "CodeTime language server")]
struct Args {
    /// Display name of the project (defaults to the workspace folder name).
    #[arg(long)]
    project: Option<String>,
    /// Absolute path of the workspace folder, used to compute relative paths.
    #[arg(long)]
    project_folder: Option<String>,
}

#[derive(Default)]
struct Settings {
    token: Option<String>,
    api_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventLog {
    project: String,
    language: String,
    relative_file: String,
    absolute_file: String,
    editor: String,
    platform: String,
    event_time: u64,
    event_type: String,
    platform_arch: String,
    git_origin: String,
    git_branch: String,
    operation_type: String,
}

struct CodetimeLanguageServer {
    client: Client,
    http: reqwest::Client,
    settings: Mutex<Settings>,
    project: String,
    project_folder: String,
    platform: String,
    platform_arch: String,
    /// Set from the LSP `clientInfo` on initialize, e.g. `Zed/0.1`.
    editor: Mutex<String>,
    /// (last file path, time it was reported) — drives heartbeat throttling.
    last: Mutex<(String, Instant)>,
}

/// Turn a `file://` URI into an OS path string.
fn uri_to_path(uri: &Url) -> String {
    uri.to_file_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|()| uri.path().to_string())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn os_display_name() -> String {
    match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
    .to_string()
}

impl CodetimeLanguageServer {
    fn relative_file(&self, absolute: &str) -> String {
        if self.project_folder.is_empty() {
            return absolute.to_string();
        }
        match absolute.strip_prefix(&self.project_folder) {
            Some(rest) => {
                let rest = rest.trim_start_matches(['/', '\\']);
                if rest.is_empty() {
                    "[other workspace]".to_string()
                } else {
                    rest.to_string()
                }
            }
            // File outside the workspace folder.
            None => "[other workspace]".to_string(),
        }
    }

    async fn git(&self, args: &[&str]) -> String {
        if self.project_folder.is_empty() {
            return String::new();
        }
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&self.project_folder)
            .args(args)
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => String::new(),
        }
    }

    async fn send(&self, uri: &Url, language: Option<String>, event_type: &str, is_write: bool) {
        let absolute_file = uri_to_path(uri);

        // Throttle repeated read events on the same file.
        {
            let mut last = self.last.lock().await;
            if !is_write && last.0 == absolute_file && last.1.elapsed() < HEARTBEAT_INTERVAL {
                return;
            }
            last.0 = absolute_file.clone();
            last.1 = Instant::now();
        }

        let (token, api_url) = {
            let settings = self.settings.lock().await;
            let token = settings
                .token
                .clone()
                .or_else(|| std::env::var("CODETIME_TOKEN").ok())
                .filter(|t| !t.is_empty());
            let api_url = settings
                .api_url
                .clone()
                .unwrap_or_else(|| DEFAULT_API_URL.to_string());
            (token, api_url)
        };

        let Some(token) = token else {
            self.client
                .log_message(
                    MessageType::WARNING,
                    "CodeTime: no token configured (set lsp.CodeTime.initialization_options.token \
                     or the CODETIME_TOKEN environment variable). Skipping event.",
                )
                .await;
            return;
        };

        let payload = EventLog {
            project: self.project.clone(),
            language: language.unwrap_or_default(),
            relative_file: self.relative_file(&absolute_file),
            absolute_file,
            editor: self.editor.lock().await.clone(),
            platform: self.platform.clone(),
            event_time: now_millis(),
            event_type: event_type.to_string(),
            platform_arch: self.platform_arch.clone(),
            git_origin: self.git(&["remote", "get-url", "origin"]).await,
            git_branch: self.git(&["rev-parse", "--abbrev-ref", "HEAD"]).await,
            operation_type: if is_write { "write" } else { "read" }.to_string(),
        };

        let url = format!("{}{}", api_url.trim_end_matches('/'), EVENT_PATH);
        let http = self.http.clone();
        let client = self.client.clone();

        // Fire-and-forget so we never block the LSP message loop on the network.
        tokio::spawn(async move {
            match http
                .post(&url)
                .bearer_auth(&token)
                .header("User-Agent", "CodeTime Zed Client")
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) if resp.status().as_u16() == 401 => {
                    client
                        .log_message(MessageType::ERROR, "CodeTime: token rejected (401).")
                        .await;
                }
                Ok(resp) => {
                    client
                        .log_message(
                            MessageType::ERROR,
                            format!("CodeTime: server returned HTTP {}.", resp.status()),
                        )
                        .await;
                }
                Err(err) => {
                    client
                        .log_message(MessageType::ERROR, format!("CodeTime: request failed: {err}"))
                        .await;
                }
            }
        });
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for CodetimeLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(info) = params.client_info {
            let editor = match info.version {
                Some(version) => format!("{}/{}", info.name, version),
                None => info.name,
            };
            *self.editor.lock().await = editor;
        }

        if let Some(options) = params.initialization_options {
            let options: Value = options;
            let mut settings = self.settings.lock().await;
            // Accept a few key spellings for convenience.
            for key in ["token", "api_key", "api-key"] {
                if let Some(v) = options.get(key).and_then(Value::as_str) {
                    settings.token = Some(v.to_string());
                    break;
                }
            }
            for key in ["api_url", "api-url"] {
                if let Some(v) = options.get(key).and_then(Value::as_str) {
                    settings.api_url = Some(v.to_string());
                    break;
                }
            }
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "CodeTime language server initialized.")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.send(&doc.uri, Some(doc.language_id), EVENT_ACTIVATE_FILE_CHANGED, false)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.send(&params.text_document.uri, None, EVENT_FILE_EDITED, true)
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.send(&params.text_document.uri, None, EVENT_FILE_SAVED, true)
            .await;
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let project_folder = args.project_folder.unwrap_or_default();
    let project = args.project.unwrap_or_else(|| {
        std::path::Path::new(&project_folder)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    });

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    let (service, socket) = LspService::new(|client| {
        Arc::new(CodetimeLanguageServer {
            client,
            http,
            settings: Mutex::new(Settings::default()),
            project,
            project_folder,
            platform: os_display_name(),
            platform_arch: std::env::consts::ARCH.to_string(),
            editor: Mutex::new("Zed".to_string()),
            last: Mutex::new((String::new(), Instant::now())),
        })
    });

    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}
