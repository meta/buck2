/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::FlameGraph;
use allocative::FlameGraphBuilder;
use allocative::FlameGraphOutput;
use allocative::Key;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::EventDispatcher;
use buck2_fs::async_fs_util::spawn_blocking;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_util::process_stats::process_stats;

use crate::daemon::server::BuckdServerData;
use crate::daemon::state::RepoState;
use crate::jemalloc_stats::get_allocator_stats;

/// In `FlameGraph` nodes do not have names, only child keys.
/// This helper makes it a bit easier to manipulate the tree.
struct NamedFlameGraph {
    name: allocative::Key,
    flamegraph: FlameGraph,
}

impl NamedFlameGraph {
    #[must_use]
    fn layer_on_top(self, name: &'static str, size: usize) -> NamedFlameGraph {
        // Each layer should be larger than the previous,
        // but if it is not, round up the difference to zero.
        let rem_size = size.saturating_sub(self.flamegraph.total_size());
        let mut new_fg = FlameGraph::default();
        new_fg.add_self(rem_size);
        new_fg.add_child(self.name, self.flamegraph);
        NamedFlameGraph {
            name: allocative::Key::new(name),
            flamegraph: new_fg,
        }
    }

    fn into_flamegraph(self) -> FlameGraph {
        let mut new_fg = FlameGraph::default();
        new_fg.add_child(self.name, self.flamegraph);
        new_fg
    }
}

fn wrap_flamegraph_with_system_stats(fg: &FlameGraph) -> FlameGraph {
    let mut fg = NamedFlameGraph {
        name: allocative::Key::new("allocative"),
        flamegraph: fg.clone(),
    };

    if let Ok(stats) = get_allocator_stats() {
        // This is technically incorrect because not everything allocative visits
        // is allocated by malloc, but it's close enough.
        if let Some(allocated) = stats.bytes_allocated {
            fg = fg.layer_on_top("jemalloc.allocated", allocated as usize);
        }
        if let Some(active) = stats.bytes_active {
            fg = fg.layer_on_top("jemalloc.active", active as usize);
        }
    }

    if let Some(rss_bytes) = process_stats().rss_bytes {
        // This is also incorrect because memory can be swapped out.
        fg = fg.layer_on_top("rss", rss_bytes as usize);
    }

    fg.into_flamegraph()
}

fn add_deferred_materializer_profile(
    mut flamegraph: FlameGraph,
    deferred_materializer: &FlameGraph,
) -> FlameGraph {
    flamegraph.add_child(
        Key::new("deferred_materializer"),
        deferred_materializer.clone(),
    );
    flamegraph
}

fn combined_warnings(
    allocative: &FlameGraphOutput,
    deferred_materializer: &FlameGraphOutput,
) -> String {
    let mut warnings = allocative.warnings();
    let deferred_materializer_warnings = deferred_materializer.warnings();
    if !deferred_materializer_warnings.is_empty() {
        if !warnings.is_empty() {
            warnings.push('\n');
        }
        warnings.push_str(&deferred_materializer_warnings);
    }
    warnings
}

pub(crate) async fn spawn_allocative(
    buckd_server_data: Arc<BuckdServerData>,
    repo: Arc<RepoState>,
    path: AbsPathBuf,
    dispatcher: EventDispatcher,
) -> buck2_error::Result<()> {
    let materializer = repo.materializer.clone();
    dispatcher.console_message("Visiting deferred materializer...".to_owned());
    let deferred_materializer_profile = materializer.allocative().await?;

    spawn_blocking(move || {
        let mut graph = FlameGraphBuilder::default();
        dispatcher.console_message(
            "Starting allocative profiling. It may take a while to finish...".to_owned(),
        );
        // TODO(nga): Emit some progress.
        dispatcher.console_message("Visiting global roots...".to_owned());
        graph.visit_global_roots();
        dispatcher.console_message("Visiting buckd...".to_owned());
        graph.visit_root(&buckd_server_data);
        let fg = graph.finish();
        let flamegraph = add_deferred_materializer_profile(
            fg.flamegraph().clone(),
            deferred_materializer_profile.flamegraph(),
        );
        let warnings = combined_warnings(&fg, &deferred_materializer_profile);
        // input path from --output-path
        fs_util::create_dir_if_not_exists(&path).categorize_input()?;
        dispatcher.console_message(format!("Writing allocative to `{}`...", path.display()));
        let final_fg = wrap_flamegraph_with_system_stats(&flamegraph);
        fs_util::write(path.join("flamegraph.src"), final_fg.write()).categorize_internal()?;
        let mut fg_svg = Vec::new();
        let mut options = inferno::flamegraph::Options::default();
        options.title = "Flame Graph - Allocative".to_owned();
        inferno::flamegraph::from_reader(&mut options, final_fg.write().as_bytes(), &mut fg_svg)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?;

        fs_util::write(path.join("flame.src"), final_fg.write().as_bytes())
            .categorize_internal()?;
        fs_util::write(path.join("flame.svg"), &fg_svg).categorize_internal()?;
        fs_util::write(path.join("flame_warnings.txt"), warnings).categorize_internal()?;

        dispatcher.console_message(format!("Allocative profile written to {}", path.display()));

        buck2_error::Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_deferred_materializer_profile() {
        let base = FlameGraph::default();
        let mut deferred_materializer = FlameGraph::default();
        deferred_materializer.add_self(5);

        let merged = add_deferred_materializer_profile(base, &deferred_materializer);

        assert!(
            merged.write().contains("deferred_materializer 5"),
            "merged flamegraph: {}",
            merged.write()
        );
    }
}
