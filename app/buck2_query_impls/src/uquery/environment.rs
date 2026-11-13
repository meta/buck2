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
use std::sync::Mutex;
use std::time::Instant;

use async_trait::async_trait;
use buck2_build_api::build_signals::HasBuildSignals;
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::file_ops::error::FileReadErrorContext;
use buck2_common::package_listing::dice::DicePackageListingResolver;
use buck2_common::package_listing::resolver::PackageListingResolver;
use buck2_common::pattern::resolve::ResolvedPattern;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::deprecation;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::environment::QueryEnvironment;
use buck2_query::query::environment::QueryEnvironmentAsNodeLookup;
use buck2_query::query::environment::QueryTarget;
use buck2_query::query::graph::node::LabeledNode;
use buck2_query::query::graph::node::NodeKey;
use buck2_query::query::graph::successors::AsyncChildVisitor;
use buck2_query::query::syntax::simple::eval::error::QueryError;
use buck2_query::query::syntax::simple::eval::file_set::FileNode;
use buck2_query::query::syntax::simple::eval::file_set::FileSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_query::query::syntax::simple::functions::DefaultQueryFunctionsModule;
use buck2_query::query::syntax::simple::functions::HasModuleDescription;
use buck2_query::query::syntax::simple::functions::docs::QueryEnvironmentDescription;
use buck2_query::query::traversal::AsyncNodeLookup;
use buck2_query::query::traversal::ChildVisitor;
use buck2_query::query::traversal::async_depth_first_postorder_traversal;
use buck2_query::query::traversal::async_depth_limited_traversal;
use buck2_util::future::try_join_all;
use derive_more::Display;
use dice::DiceComputations;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use itertools::Itertools;
use ref_cast::RefCast;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum QueryLiteralResolutionError {
    #[error("literal `{0}` missing in pre-resolved literals")]
    LiteralMissing(String),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum RBuildFilesError {
    #[error("no parent found for the file `{0}`")]
    ParentDoesNotExist(CellPath),
    #[error("internal error: no buildfile names found for the cell `{0}`")]
    CellMissingBuildFileNames(CellName),
}

/// UqueryDelegate resolves information needed by the QueryEnvironment.
#[async_trait]
pub(crate) trait UqueryDelegate: Send + Sync {
    async fn get_buildfile_names_by_cell(
        &self,
    ) -> buck2_error::Result<BuckMutMap<CellName, Arc<[FileNameBuf]>>>;

    /// Resolves a target pattern.
    async fn resolve_target_patterns(
        &self,
        pattern: &[&str],
    ) -> buck2_error::Result<ResolvedPattern<TargetPatternExtra>>;

    async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet>;

    // Every package enclosing the path, not only the nearest one, since a target may name
    // sources from a directory that is itself a package.
    async fn get_enclosing_packages(
        &self,
        path: &CellPath,
    ) -> buck2_error::Result<Vec<PackageLabel>>;

    fn linear_dice_computations(&self) -> LinearRecomputeDiceComputations<'_, '_>;

    fn ctx(&self) -> DiceComputations<'_>;
}

#[async_trait]
pub(crate) trait QueryLiterals<T: QueryTarget>: Send + Sync {
    async fn eval_literals(
        &self,
        literals: &[&str],
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetSet<T>>;
}

pub(crate) struct UqueryEnvironment<'c> {
    delegate: &'c dyn UqueryDelegate,
    literals: Arc<dyn QueryLiterals<TargetNode> + 'c>,
    allow_partial_graph: bool,
    /// Packages whose load this environment has already credited to a
    /// requester, or that a literal loaded; only the first request for a
    /// package is the one that started its load.
    reported_loads: Mutex<BuckMutSet<PackageLabel>>,
}

pub(crate) struct PreresolvedQueryLiterals<T: QueryTarget> {
    resolved_literals: BuckMutMap<String, buck2_error::Result<TargetSet<T>>>,
}

impl<T: QueryTarget> PreresolvedQueryLiterals<T> {
    pub(crate) fn new(
        resolved_literals: BuckMutMap<String, buck2_error::Result<TargetSet<T>>>,
    ) -> Self {
        Self { resolved_literals }
    }

    pub(crate) async fn pre_resolve(
        base: &dyn QueryLiterals<T>,
        literals: &[String],
        dice: &mut DiceComputations<'_>,
    ) -> Self {
        let resolved_literal_results = dice
            .compute_join(literals.iter(), async |ctx, lit| {
                (lit.to_owned(), base.eval_literals(&[lit], ctx).await)
            })
            .await;
        let mut resolved_literals = BuckMutMap::default();
        for (literal, result) in resolved_literal_results {
            resolved_literals.insert(literal, result);
        }
        Self { resolved_literals }
    }

    /// All the literals, or error if resolution of any failed.
    pub(crate) fn literals(&self) -> buck2_error::Result<TargetSet<T>> {
        let mut literals = TargetSet::new();
        for result in self.resolved_literals.values() {
            literals.extend(result.as_ref().map_err(|e| e.dupe())?);
        }
        Ok(literals)
    }
}

impl PreresolvedQueryLiterals<TargetNode> {
    /// Packages of the targets the literals resolved to.
    pub(crate) fn packages(&self) -> BuckMutSet<PackageLabel> {
        let mut packages = BuckMutSet::default();
        let mut last: Option<PackageLabel> = None;
        for targets in self
            .resolved_literals
            .values()
            .filter_map(|r| r.as_ref().ok())
        {
            for target in targets.iter() {
                let pkg = target.label().pkg();
                // Targets of one package are adjacent; skip rehashing them.
                if last.as_ref() != Some(&pkg) {
                    packages.insert(pkg.dupe());
                    last = Some(pkg);
                }
            }
        }
        packages
    }
}

#[async_trait]
impl<T: QueryTarget> QueryLiterals<T> for PreresolvedQueryLiterals<T> {
    async fn eval_literals(
        &self,
        literals: &[&str],
        _: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetSet<T>> {
        let mut targets = TargetSet::new();
        for lit in literals {
            let resolved = match self
                .resolved_literals
                .get(*lit)
                .ok_or_else(|| QueryLiteralResolutionError::LiteralMissing((*lit).to_owned()))?
            {
                Ok(v) => v,
                Err(e) => return Err(e.dupe()),
            };
            targets.extend(resolved);
        }
        Ok(targets)
    }
}

impl<'c> UqueryEnvironment<'c> {
    pub(crate) fn new(
        delegate: &'c dyn UqueryDelegate,
        literals: Arc<dyn QueryLiterals<TargetNode> + 'c>,
        literal_packages: BuckMutSet<PackageLabel>,
        allow_partial_graph: bool,
    ) -> Self {
        Self {
            delegate,
            literals,
            allow_partial_graph,
            reported_loads: Mutex::new(literal_packages),
        }
    }

    pub(crate) fn describe() -> QueryEnvironmentDescription {
        QueryEnvironmentDescription {
            name: "Uquery Environment".to_owned(),
            mods: vec![DefaultQueryFunctionsModule::<Self>::describe()],
        }
    }

    async fn get_node(&self, target: &TargetLabel) -> buck2_error::Result<TargetNode> {
        let package = self
            .delegate
            .ctx()
            .get_interpreter_results(target.pkg())
            .await
            .with_buck_error_context(|| format!("Error looking up `{target}`"))?;
        let node = package.resolve_target(target.name())?;
        Ok(node.to_owned())
    }
}

#[async_trait]
impl QueryEnvironment for UqueryEnvironment<'_> {
    type Target = TargetNode;

    fn allow_partial_graph(&self) -> bool {
        self.allow_partial_graph
    }

    async fn get_node(&self, node_ref: &TargetLabel) -> buck2_error::Result<Self::Target> {
        UqueryEnvironment::get_node(self, node_ref).await
    }

    async fn get_node_from(
        &self,
        node_ref: &TargetLabel,
        requester: Option<&TargetLabel>,
    ) -> buck2_error::Result<Self::Target> {
        // The traversal, not a tracked computation, is what asks for this package, so
        // tell build signals which node's expansion requested it, once the request returns.
        let report = requester.filter(|from| {
            from.pkg() != node_ref.pkg()
                && self.reported_loads.lock().unwrap().insert(node_ref.pkg())
        });
        let requested_at = Instant::now();
        let node = UqueryEnvironment::get_node(self, node_ref).await;
        if let Some(from) = report {
            let ctx = self.delegate.ctx();
            if let Some(signals) = ctx.per_transaction_data().get_build_signals() {
                signals.load_discovered(node_ref.pkg(), from.pkg(), requested_at);
            }
        }
        node
    }

    async fn get_node_for_default_configured_target(
        &self,
        _node_ref: &TargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<Self::Target>> {
        Err(QueryError::FunctionUnimplemented(
            "get_node_for_default_configured_target() only for CqueryEnvironment",
        )
        .into())
    }

    async fn eval_literals(&self, literals: &[&str]) -> buck2_error::Result<TargetSet<TargetNode>> {
        self.literals
            .eval_literals(literals, &mut self.delegate.ctx())
            .await
    }

    async fn eval_file_literal(&self, literal: &str) -> buck2_error::Result<FileSet> {
        self.delegate.eval_file_literal(literal).await
    }

    async fn dfs_postorder(
        &self,
        root: &TargetSet<TargetNode>,
        traversal_delegate: impl AsyncChildVisitor<TargetNode>,
        visit: impl FnMut(TargetNode) -> buck2_error::Result<()> + Send,
    ) -> buck2_error::Result<()> {
        async_depth_first_postorder_traversal(
            &QueryEnvironmentAsNodeLookup { env: self },
            root.iter_names(),
            traversal_delegate,
            visit,
            self.allow_partial_graph,
        )
        .await
    }

    async fn depth_limited_traversal(
        &self,
        root: &TargetSet<Self::Target>,
        delegate: impl AsyncChildVisitor<Self::Target>,
        visit: impl FnMut(Self::Target) -> buck2_error::Result<()> + Send,
        depth: u32,
    ) -> buck2_error::Result<()> {
        async_depth_limited_traversal(
            &QueryEnvironmentAsNodeLookup { env: self },
            root.iter_names(),
            delegate,
            visit,
            depth,
            self.allow_partial_graph,
        )
        .await
    }

    async fn allbuildfiles(
        &self,
        universe: &TargetSet<Self::Target>,
    ) -> buck2_error::Result<FileSet> {
        return allbuildfiles(universe, self.delegate).await;
    }

    async fn rbuildfiles(
        &self,
        universe: &FileSet,
        argset: &FileSet,
    ) -> buck2_error::Result<FileSet> {
        return rbuildfiles(universe, argset, self.delegate).await;
    }

    async fn owner(&self, paths: &FileSet) -> buck2_error::Result<TargetSet<Self::Target>> {
        validate_owner_paths(&mut self.delegate.ctx(), paths).await?;
        let mut result: TargetSet<Self::Target> = TargetSet::new();
        for path in paths.iter() {
            // need to explicitly track this rather than checking for changes to result set since the owner might
            // already be in the set.
            match self.delegate.get_enclosing_packages(path).await {
                Ok(packages) => {
                    let package_futs = packages
                        .iter()
                        .map(|package| async move {
                            let targets = self
                                .delegate
                                .ctx()
                                .get_interpreter_results(package.dupe())
                                .await?;

                            let owner_targets: Vec<Self::Target> = targets
                                .targets()
                                .values()
                                .filter_map(|node| {
                                    for input in node.inputs() {
                                        if &input == path {
                                            return Some(node.to_owned());
                                            // this intentionally breaks out of the loop. We don't need to look at the
                                            // other inputs of this target, but it's possible for a single file to be owned by
                                            // multiple targets.
                                        }
                                    }
                                    None
                                })
                                .collect();
                            buck2_error::Ok(owner_targets)
                        })
                        .collect::<Vec<_>>();

                    for nodes in buck2_util::future::join_all(package_futs).await.into_iter() {
                        for node in nodes?.into_iter() {
                            result.insert(node);
                        }
                    }
                }
                Err(_) => {
                    // we don't consider this an error, it's usually the case that the user
                    // just wants to know the target owning the file if it exists.
                }
            };
        }
        Ok(result)
    }

    /// Finds all targets in some buildfiles.
    ///
    /// todo(dbarsky): instead of having this be a trait method, this should be implemented as a uquery-specific module
    /// similar to [crate::aquery::functions::AqueryFunctions].
    async fn targets_in_buildfile(
        &self,
        paths: &FileSet,
    ) -> buck2_error::Result<TargetSet<Self::Target>> {
        let mut result: TargetSet<Self::Target> = TargetSet::new();

        let compute = &mut self.delegate.ctx();
        let mut resolver = DicePackageListingResolver(compute);

        let mut buildfiles = vec![];
        for path in paths.iter() {
            let package_label = resolver.get_enclosing_package(*path).await?;
            let listing = resolver.resolve(package_label.dupe()).await?;

            // the listing's buildfile is relative to the package (the BUCK/TARGETS file).
            // this makes it actually relative to the cell.
            let buildfile = package_label.as_cell_path().join(listing.buildfile());
            if buildfile != *path {
                // file is a not a buildfile.
                continue;
            }
            buildfiles.push(package_label);
        }

        let mut evaluations = vec![];
        for buildfile in buildfiles {
            let fut = async move { self.delegate.ctx().get_interpreter_results(buildfile).await };
            evaluations.push(fut);
        }

        let targets = try_join_all(evaluations).await?;
        for evaluation_result in targets {
            result.extend(
                evaluation_result
                    .targets()
                    .values()
                    .map(|target_ref| target_ref.to_owned()),
            );
        }

        Ok(result)
    }
}

pub(crate) async fn validate_owner_paths(
    ctx: &mut DiceComputations<'_>,
    paths: &FileSet,
) -> buck2_error::Result<()> {
    for path in paths.iter() {
        if let Err(error) = DiceFileComputations::read_path_metadata(ctx, *path)
            .await
            .without_package_context_information()
        {
            deprecation!("owner_invalid_path", error, owner: "ianc", started: "2026-10-12");
        }
    }
    Ok(())
}

pub(crate) async fn allbuildfiles<T: QueryTarget>(
    universe: &TargetSet<T>,
    delegate: &dyn UqueryDelegate,
) -> buck2_error::Result<FileSet> {
    let mut paths = BuckMutInsertionSet::<FileNode>::default();

    let mut top_level_imports = Vec::<ImportPath>::new();

    for target in universe.iter() {
        paths.insert(FileNode(target.buildfile_path()?.path()));

        let eval_result = delegate
            .ctx()
            .get_interpreter_results(target.buildfile_path()?.package())
            .await?; // TODO: no longer use eval_build_file, just parse imports directly (will solve async issue too)

        top_level_imports.extend(eval_result.imports().iter().cloned());
    }

    let loads =
        get_transitive_loads(top_level_imports, delegate.linear_dice_computations()).await?;

    let mut new_paths = BuckMutInsertionSet::<FileNode>::default();
    for load in &loads {
        new_paths.insert(FileNode(*load.path()));
    }

    Ok(FileSet::new(paths).union(&FileSet::new(new_paths)))
}

pub(crate) async fn rbuildfiles(
    universe: &FileSet,
    argset: &FileSet,
    delegate: &dyn UqueryDelegate,
) -> buck2_error::Result<FileSet> {
    let universe_paths: Vec<CellPath> = universe.iter().copied().collect();
    // step 1: split the build files and bzl files
    let (buildfiles, bzlfiles) = split_universe_files(&universe_paths, delegate).await?;

    // step 2: get all top level imports accordingly
    let top_level_import_by_build_file =
        top_level_imports_by_build_file(&buildfiles, &bzlfiles, delegate).await?;

    // step 3: get the first order imports for every loaded file, to lookup during traversal
    // TODO(benfoxman) this is actually unnecessary, since we can get the imports while traversing.
    // Will fix this in a separate diff.
    let all_top_level_imports: Vec<ImportPath> = top_level_import_by_build_file
        .values()
        .flat_map(|imports| imports.iter().cloned())
        .collect();

    let first_order_import_map = first_order_imports(&all_top_level_imports, delegate).await?;

    // step 4: do rdeps()-like traversal from the top level imports down to bzls

    #[derive(Clone, Dupe)]
    struct Node(Arc<ImportPath>);

    impl Node {
        fn import_path(&self) -> &ImportPath {
            &self.0
        }
    }

    #[derive(Display, Debug, Hash, Eq, PartialEq, Clone, RefCast)]
    #[repr(transparent)]
    struct NodeRef(ImportPath);

    impl NodeKey for NodeRef {}

    impl LabeledNode for Node {
        type Key = NodeRef;

        fn node_key(&self) -> &NodeRef {
            NodeRef::ref_cast(self.import_path())
        }
    }

    struct Lookup {}

    #[async_trait]
    impl AsyncNodeLookup<Node> for Lookup {
        async fn get(&self, label: &NodeRef) -> buck2_error::Result<Node> {
            Ok(Node(Arc::new(label.0.clone())))
        }
    }

    let mut output_paths: Vec<ImportPath> = Vec::new();

    struct Delegate<'a> {
        first_order_import_map: &'a BuckMutMap<ImportPath, Vec<ImportPath>>,
    }

    let visit = |node: Node| {
        let node_import = node.import_path();
        if argset.iter().contains(node_import.path()) {
            output_paths.push(node_import.clone());
        } else {
            let loads = first_order_import_map
                .get(node_import)
                .expect("import path should exist");
            for load in loads.iter() {
                for arg in argset.iter() {
                    if load.path() == arg {
                        output_paths.push(node_import.clone());
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    };

    impl AsyncChildVisitor<Node> for Delegate<'_> {
        async fn for_each_child(
            &self,
            node: &Node,
            mut func: impl ChildVisitor<Node>,
        ) -> buck2_error::Result<()> {
            for import in self
                .first_order_import_map
                .get(node.import_path())
                .expect("import path should exist")
            {
                func.visit(&NodeRef(import.clone()))?;
            }
            Ok(())
        }
    }

    let lookup = Lookup {};
    let delegate = Delegate {
        first_order_import_map: &first_order_import_map,
    };

    // step 5: do traversal, get all modified imports
    async_depth_first_postorder_traversal(
        &lookup,
        all_top_level_imports.iter().map(NodeRef::ref_cast),
        delegate,
        visit,
        false, // allow_partial_graph
    )
    .await?;

    let mut output_files = BuckMutInsertionSet::<FileNode>::default();
    for file in &output_paths {
        output_files.insert(FileNode(*file.path()));
    }

    // finally, since we didn't check the top level imports of buildfiles,
    // we do this right now
    for file in buildfiles {
        if argset.iter().contains(&file) {
            output_files.insert(FileNode(file));
        } else {
            let imports = top_level_import_by_build_file
                .get(&file)
                .expect("should have stored a universe file's input");

            for import in imports.iter() {
                if output_files.iter().contains(&FileNode(*import.path())) {
                    output_files.insert(FileNode(file));
                    break;
                }
            }
        }
    }

    Ok(FileSet::new(output_files))
}

async fn split_universe_files(
    universe: &[CellPath],
    delegate: &dyn UqueryDelegate,
) -> buck2_error::Result<(Vec<CellPath>, Vec<CellPath>)> {
    let mut buildfiles = Vec::<CellPath>::new();
    let mut bzlfiles = Vec::<CellPath>::new();
    let buildfile_names_by_cell = delegate.get_buildfile_names_by_cell().await?;
    for file in universe {
        let buildfile_names_for_file =
            buildfile_names_by_cell.get(&file.cell()).ok_or_else(|| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "{}",
                    RBuildFilesError::CellMissingBuildFileNames(file.cell()).to_string()
                )
            })?;

        if let Some(name) = file.path().file_name() {
            if buildfile_names_for_file
                .iter()
                .map(<FileNameBuf as AsRef<FileName>>::as_ref)
                .contains(&name)
            {
                buildfiles.push(*file);
            }
        } else {
            // TODO: right now we assume non-buildfiles are bzl's - we might want to handle error cases later.
            bzlfiles.push(*file);
        }
    }
    Ok((buildfiles, bzlfiles))
}

async fn top_level_imports_by_build_file(
    buildfiles: &[CellPath],
    bzlfiles: &[CellPath],
    delegate: &dyn UqueryDelegate,
) -> buck2_error::Result<BuckMutMap<CellPath, Vec<ImportPath>>> {
    let mut top_level_import_by_build_file = BuckMutMap::<CellPath, Vec<ImportPath>>::default();

    for file in bzlfiles {
        let imports = vec![ImportPath::new_same_cell(*file)?];
        top_level_import_by_build_file.insert(*file, imports);
    }

    let mut buildfile_futs: FuturesUnordered<_> = buildfiles
        .iter()
        .map(|file| async move {
            if let Some(parent) = file.parent() {
                match PackageLabel::from_cell_path(parent) {
                    Ok(label) => (*file, delegate.ctx().get_interpreter_results(label).await),
                    Err(e) => (*file, Err(e)),
                }
            } else {
                (
                    *file,
                    Err(RBuildFilesError::ParentDoesNotExist(*file).into()),
                )
            }
        })
        .collect();

    while let Some((file, eval_result)) = tokio::task::unconstrained(buildfile_futs.next()).await {
        top_level_import_by_build_file.insert(file, eval_result?.imports().to_owned());
    }

    Ok(top_level_import_by_build_file)
}

// TODO: no need to get all the first order imports prior to the traversal - we can do this
// at the same time as the actual traversal.
async fn first_order_imports(
    all_top_level_imports: &[ImportPath],
    delegate: &dyn UqueryDelegate,
) -> buck2_error::Result<BuckMutMap<ImportPath, Vec<ImportPath>>> {
    let all_imports = get_transitive_loads(
        all_top_level_imports.to_vec(),
        delegate.linear_dice_computations(),
    )
    .await?;

    let mut all_first_order_futs: FuturesUnordered<_> = all_imports
        .iter()
        .map(|node| async move { (node, delegate.ctx().get_loaded_module_imports(node).await) })
        .collect();

    let mut first_order_import_map = BuckMutMap::<ImportPath, Vec<ImportPath>>::default();

    while let Some((import, first_order_imports)) =
        tokio::task::unconstrained(all_first_order_futs.next()).await
    {
        let prev = first_order_import_map.insert(import.clone(), first_order_imports?);
        assert!(prev.is_none());
    }
    Ok(first_order_import_map)
}

// Uquery and Cquery share ImportPath traversal logic, so we move the logic to this function.
pub(crate) async fn get_transitive_loads(
    top_level_imports: Vec<ImportPath>,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
) -> buck2_error::Result<Vec<ImportPath>> {
    #[derive(Clone, Dupe)]
    struct Node(Arc<ImportPath>);

    impl Node {
        fn import_path(&self) -> &ImportPath {
            &self.0
        }
    }

    #[derive(Display, Debug, Hash, Eq, PartialEq, Clone, RefCast)]
    #[repr(transparent)]
    struct NodeRef(ImportPath);

    impl NodeKey for NodeRef {}

    impl LabeledNode for Node {
        type Key = NodeRef;

        fn node_key(&self) -> &NodeRef {
            NodeRef::ref_cast(self.import_path())
        }
    }

    struct Lookup {}

    #[async_trait]
    impl AsyncNodeLookup<Node> for Lookup {
        async fn get(&self, label: &NodeRef) -> buck2_error::Result<Node> {
            Ok(Node(Arc::new(label.0.clone())))
        }
    }

    let mut imports: Vec<ImportPath> = Vec::new();

    struct Delegate<'c, 'a> {
        ctx: LinearRecomputeDiceComputations<'c, 'a>,
    }

    let visit = |target: Node| {
        imports.push(target.import_path().clone());
        Ok(())
    };

    impl AsyncChildVisitor<Node> for Delegate<'_, '_> {
        async fn for_each_child(
            &self,
            target: &Node,
            mut func: impl ChildVisitor<Node>,
        ) -> buck2_error::Result<()> {
            for import in self
                .ctx
                .get()
                .get_loaded_module_imports(target.import_path())
                .await?
            {
                func.visit(&NodeRef(import))?;
            }
            Ok(())
        }
    }

    let traversal_delegate = Delegate { ctx };
    let lookup = Lookup {};

    let import_nodes = top_level_imports.iter().map(NodeRef::ref_cast);

    async_depth_first_postorder_traversal(&lookup, import_nodes, traversal_delegate, visit, false)
        .await?;

    Ok(imports)
}
