pub mod collect;
pub mod run;
pub mod notify;

pub mod psyop;
pub mod sort_by;
pub mod filter;
pub mod output_top;

// Type definitions + publish-time validators moved to the SDK
// under `psychological_operations_sdk::cli::psyops`. Re-export at
// the same shorthand `crate::psyops::*` so call sites that wrote
// `use crate::psyops::PsyOp;` keep resolving.
pub use psychological_operations_sdk::cli::psyops::{
    Filter, ForYou, OutputTop, PsyOp, PsyopEntry, PublishedPsyop, Query,
    SearchEndpoint, SortBy, Stage, StageBase, is_vector_function,
};

use clap::Args;
use psychological_operations_sdk::cli::Output;

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct PsyopSource {
    /// Inline JSON psyop definition
    #[arg(long)]
    psyop_inline: Option<String>,
    /// Path to a JSON file containing the psyop definition
    #[arg(long)]
    psyop_file: Option<std::path::PathBuf>,
}

#[derive(Args)]
pub struct PublishArgs {
    /// Psyop name
    #[arg(long)]
    pub name: String,
    #[command(flatten)]
    pub source: PsyopSource,
}

pub(crate) async fn list(
    enabled: bool,
    disabled: bool,
    count: Option<usize>,
    offset: Option<usize>,
    ctx: &crate::context::Context,
) -> bool {
    crate::output::emit_result(list_inner(enabled, disabled, count, offset, ctx).await)
}

async fn list_inner(
    enabled: bool,
    disabled: bool,
    count: Option<usize>,
    offset: Option<usize>,
    ctx: &crate::context::Context,
) -> Result<Output, crate::error::Error> {
    let mut entries: Vec<PsyopEntry> = ctx
        .db
        .psyop_list()
        .await?
        .into_iter()
        .filter_map(|(name, _def, is_disabled)| {
            let is_enabled = !is_disabled;
            if enabled && !is_enabled {
                return None;
            }
            if disabled && is_enabled {
                return None;
            }
            Some(PsyopEntry { name, enabled: is_enabled })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let start = offset.unwrap_or(0);
    let end = match count {
        Some(c) => start.saturating_add(c).min(entries.len()),
        None    => entries.len(),
    };
    let page: &[PsyopEntry] = if start >= entries.len() {
        &[]
    } else {
        &entries[start..end]
    };
    Ok(Output::PsyopList(page.to_vec()))
}

/// Emit the JSON Schema for [`PsyOp`] so agents / operators can
/// see what shape `psyops publish --psyop-inline '<json>'`
/// accepts. No ctx — pure type derivation.
pub(crate) fn schema() -> bool {
    crate::output::emit_result((|| -> Result<Output, crate::error::Error> {
        Ok(Output::Schema(schemars::schema_for!(self::PsyOp)))
    })())
}

pub(crate) async fn get(name: &str, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(async {
        let psyop = self::psyop::load(name, ctx).await?;
        Ok::<_, crate::error::Error>(Output::Psyop(psyop))
    }.await)
}

pub(crate) async fn set_disabled(name: &str, value: bool, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(set_disabled_inner(name, value, ctx).await)
}

async fn set_disabled_inner(name: &str, value: bool, ctx: &crate::context::Context) -> Result<Output, crate::error::Error> {
    if !ctx.db.psyop_set_disabled(name, value).await? {
        return Err(crate::error::Error::PsyopNotFound(name.to_string()));
    }

    // Notify the running viewer (if any) that this psyop's surfaced
    // entry just changed. The definition is unchanged here, but the
    // entry's `enabled` flag flips — viewers re-render accordingly.
    // Best-effort; silent failures.
    if let Some(body) = full_psyop_body(name, ctx).await {
        notify::notify("psyop_edited", &body, ctx).await;
    }
    Ok(Output::Ok)
}

pub(crate) async fn publish(args: PublishArgs, ctx: &crate::context::Context) -> bool {
    crate::output::emit_result(publish_inner(args, ctx).await)
}

async fn publish_inner(args: PublishArgs, ctx: &crate::context::Context) -> Result<Output, crate::error::Error> {
    let psyop: PsyOp = if let Some(inline) = args.source.psyop_inline {
        serde_json::from_str(&inline)?
    } else if let Some(path) = args.source.psyop_file {
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data)?
    } else {
        unreachable!("clap group ensures one is set")
    };
    psyop.validate().map_err(crate::error::Error::InvalidPsyop)?;

    // Add vs edit: existence BEFORE the upsert. `psyop_upsert` leaves
    // the `disabled` flag untouched on edit.
    let existed_before = ctx.db.psyop_exists(&args.name).await?;
    self::psyop::save(&args.name, &psyop, ctx).await?;

    let is_enabled = !ctx.db.psyop_disabled(&args.name).await?;
    let body = serde_json::json!({
        "name": &args.name,
        "enabled": is_enabled,
        "definition": &psyop,
    });
    let sub_type = if existed_before { "psyop_edited" } else { "psyop_added" };
    notify::notify(sub_type, &body, ctx).await;

    Ok(Output::PublishedPsyop(PublishedPsyop {
        name: args.name,
        enabled: is_enabled,
    }))
}

/// Build the `PsyopWithDefinition`-shaped notification body for
/// `psyop_added` / `psyop_edited`. Returns `None` if the psyop can't be
/// read back — caller drops the notify.
async fn full_psyop_body(
    name: &str,
    ctx: &crate::context::Context,
) -> Option<serde_json::Value> {
    let psyop = self::psyop::load(name, ctx).await.ok()?;
    let is_enabled = !ctx.db.psyop_disabled(name).await.unwrap_or(false);
    Some(serde_json::json!({
        "name": name,
        "enabled": is_enabled,
        "definition": psyop,
    }))
}
