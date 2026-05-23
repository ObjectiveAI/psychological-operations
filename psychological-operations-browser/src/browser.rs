//! Build the single WebviewWindow that hosts the session.

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::args::Args;
use crate::pointer;

pub fn build_window<R: Runtime>(handle: &AppHandle<R>, args: &Args) -> Result<WebviewWindow<R>> {
    let target_dir = args.target_dir();
    std::fs::create_dir_all(&target_dir)
        .with_context(|| format!("creating target dir {}", target_dir.display()))?;

    let url = args.initial_url()?;

    WebviewWindowBuilder::new(handle, "main", WebviewUrl::External(url))
        .title("psychological-operations")
        .inner_size(1280.0, 800.0)
        .initialization_script(&pointer::init_script())
        .build()
        .context("building webview window")
}
