//! A fully wired [`Session`]: footage decoding through `effectcraft-media` (FilmCraft's codecs),
//! the media importer, the expression engine, JavaScript scripting (`effectcraft-script`) and
//! Render Queue export through `effectcraft-export` (craft-libs' encoders). Frontends (desktop,
//! CLI, MCP, web) start here.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use effectcraft_engine::project::render_queue::OutputFormat;
use effectcraft_engine::{ExportJob, ExportResult, Exporter, Importer, Session};
use effectcraft_project::Footage;
pub use effectcraft_script as script;

pub mod app_dirs;

struct MediaImporter;

impl Importer for MediaImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_media::probe(path).map_err(|e| e.to_string())
    }
    fn probe_noting(&self, path: &str) -> Result<(Footage, Option<String>), String> {
        effectcraft_media::probe_noting(path).map_err(|e| e.to_string())
    }
}

/// Render Queue export via `effectcraft-export`: to the file system, or to `sink` (the web app
/// turns written files into downloads).
#[derive(Default)]
pub struct FileExporter {
    pub sink: Option<Arc<effectcraft_export::Sink>>,
}

impl Exporter for FileExporter {
    fn formats(&self) -> Vec<OutputFormat> {
        effectcraft_export::available_formats()
    }
    fn supports_skip_existing(&self, settings: &effectcraft_project::render_queue::RenderSettings) -> bool {
        // The exporter's existence check precedes storage-overflow redirection.
        self.sink.is_none() && !cfg!(target_arch = "wasm32") && !settings.storage_overflow
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        effectcraft_engine::render::passes::block_on(self.export_async(job, progress))
    }

    fn export_async<'a>(
        &'a self,
        job: &'a ExportJob<'a>,
        progress: &'a mut dyn FnMut(u64, u64) -> bool,
    ) -> effectcraft_engine::render_queue::LocalFuture<'a, Result<ExportResult, String>> {
        Box::pin(async move {
            let j = effectcraft_export::Job {
                project: job.project,
                footage: job.footage,
                expr: job.expr,
                accel: job.accel,
                comp: job.item.comp,
                settings: &job.item.settings,
                output: &job.item.output,
                path: job.path,
                sink: self.sink.as_deref(),
                options: effectcraft_export::JobOptions {
                    log: job.item.log,
                    label: job.label.clone(),
                    storage: job.storage,
                    overflow: job.project.render_prefs.overflow_folders.clone(),
                    caches: job.caches.map(|c| effectcraft_export::JobCaches { gpu: &c.gpu, cpu: &c.cpu }),
                    renderer: job.renderer,
                    gpu_frames: job.gpu_frames,
                    free_memory: Some(&free_memory),
                },
                nested_switches: job.nested_switches,
            };
            match effectcraft_export::export_async(&j, &mut |p| progress(p.done, p.total)).await {
                Ok(r) => Ok(ExportResult {
                    path: r.path,
                    frames: r.frames,
                    width: r.width,
                    height: r.height,
                    bytes: r.bytes,
                    seconds: r.seconds,
                    audio: r.audio,
                    log: r.log,
                    overflow: r.overflow,
                }),
                Err(effectcraft_export::ExportError::Cancelled) => Err(effectcraft_engine::render_queue::CANCELLED.into()),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// The memory the system can hand out now, for an export's Auto CPU frames in flight (#1615).
fn free_memory() -> Option<u64> {
    effectcraft_engine::sysinfo::memory().map(|m| m.available)
}

/// A new session with media, import, expressions, scripting and export enabled.
pub fn session() -> Session {
    let mut s = Session {
        // The desktop app, the CLI and the MCP server share installed Roto Brush models.
        models_dir: config_dir().map(|d| d.join("models")),
        exporter: Some(Arc::new(FileExporter::default())),
        footage: Arc::new(effectcraft_media::MediaPool::new()),
        importer: Some(Arc::new(MediaImporter)),
        expr: Some(Arc::new(effectcraft_expr::Expressions)),
        expr_check: Some(effectcraft_expr::check_syntax),
        ..Default::default()
    };
    effectcraft_script::install(&mut s);
    // Craft plugins: loads enabled contributions (skipped in safe mode; the manager still works).
    effectcraft_plugins::start(&mut s, safe_mode());
    s
}

/// Safe mode (`--safe-mode`, or `EFFECTCRAFT_SAFE_MODE=1`): no plugin code runs this session.
pub fn safe_mode() -> bool {
    std::env::var("EFFECTCRAFT_SAFE_MODE").is_ok_and(|v| v == "1") || std::env::args().any(|a| a == "--safe-mode")
}

/// `effectcraft-cli plugin …`: the shared Craft plugin commands for this app; returns the exit code.
#[cfg(not(target_arch = "wasm32"))]
pub fn plugin_cli(args: &[String], out: &mut dyn std::io::Write, err: &mut dyn std::io::Write) -> i32 {
    effectcraft_plugins::run_cli(args, out, err)
}

/// Stop plugin sessions and native helpers (when the app quits).
pub fn stop_plugins() {
    effectcraft_plugins::stop();
}

/// The platform config directory for EffectCraft (`EFFECTCRAFT_CONFIG_DIR` overrides):
/// `~/Library/Application Support/EffectCraft` (macOS), `%APPDATA%\EffectCraft` (Windows),
/// `$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` (Linux and others). A `portable.txt`
/// beside the executable puts it in `EffectCraftData` there instead ([`app_dirs`]).
pub fn config_dir() -> Option<std::path::PathBuf> {
    app_dirs::current().dir.clone()
}

#[cfg(test)]
mod tests;
