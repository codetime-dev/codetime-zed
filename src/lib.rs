use std::{
    env, fs,
    path::{Path, PathBuf},
};

use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

const REPO: &str = "codetime-dev/codetime-zed";
const BINARY: &str = "codetime-ls";

struct CodetimeExtension {
    cached_binary_path: Option<PathBuf>,
}

fn executable_name(binary: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => format!("{binary}.exe"),
        _ => binary.to_string(),
    }
}

/// Strip the leading slash WASI prepends to absolute Windows paths.
fn sanitize_path(path: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => path.trim_start_matches('/').to_string(),
        _ => path.to_string(),
    }
}

fn project_name_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
}

impl CodetimeExtension {
    /// Release asset triple, e.g. `codetime-ls-x86_64-unknown-linux-gnu`.
    fn target_triple(&self) -> Result<String, String> {
        let (platform, arch) = zed::current_platform();
        let arch = match arch {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X8664 => "x86_64",
            _ => return Err(format!("unsupported architecture: {arch:?}")),
        };
        let os = match platform {
            zed::Os::Mac => "apple-darwin",
            zed::Os::Linux => "unknown-linux-gnu",
            zed::Os::Windows => "pc-windows-msvc",
        };
        Ok(format!("{BINARY}-{arch}-{os}"))
    }

    fn download(&self, language_server_id: &LanguageServerId) -> Result<PathBuf> {
        let release = zed::latest_github_release(
            REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let target_triple = self.target_triple()?;
        let asset_name = format!("{target_triple}.zip");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no release asset found matching {asset_name:?}"))?;

        let version_dir = format!("{BINARY}-{}", release.version);
        let binary_path = Path::new(&version_dir).join(executable_name(BINARY));

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(
                &asset.download_url,
                &version_dir,
                zed::DownloadedFileType::Zip,
            )
            .map_err(|err| format!("failed to download {asset_name}: {err}"))?;

            // Drop older versioned download directories.
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.starts_with(BINARY) && name != version_dir {
                            fs::remove_dir_all(entry.path()).ok();
                        }
                    }
                }
            }
        }

        zed::make_file_executable(binary_path.to_str().unwrap())?;
        Ok(binary_path)
    }

    fn binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<String, String> {
        // 1. Explicit override (useful while developing the language server).
        if let Some(path) = worktree.which(&executable_name(BINARY)) {
            return Ok(path);
        }

        // 2. Previously downloaded binary.
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.to_string_lossy().into_owned());
            }
        }

        // 3. Download from GitHub releases, resolving to an absolute path so the
        //    language server launches regardless of Zed's working directory.
        let relative = self.download(language_server_id)?;
        let absolute = env::current_dir()
            .map(|cwd| cwd.join(&relative))
            .unwrap_or(relative);
        self.cached_binary_path = Some(absolute.clone());
        Ok(sanitize_path(&absolute.to_string_lossy()))
    }
}

impl zed::Extension for CodetimeExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let command = self.binary_path(language_server_id, worktree)?;

        let mut args = Vec::new();
        let project_folder = sanitize_path(worktree.root_path().as_str());
        if !project_folder.is_empty() {
            if let Some(project_name) = project_name_from_path(&project_folder) {
                args.push("--project".to_string());
                args.push(project_name);
            }
            args.push("--project-folder".to_string());
            args.push(project_folder);
        }

        Ok(Command {
            command,
            args,
            // Forward the shell environment so the server can pick up
            // CODETIME_TOKEN / HTTPS_PROXY without extra configuration.
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(CodetimeExtension);
