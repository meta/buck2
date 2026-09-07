/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::DigestAlgorithm;
use buck2_common::external_symlink::ExternalSymlink;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::Symlink;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::builder::DirectoryBuilder;
use buck2_directory::directory::builder_lazy::DirectoryBuilderLike;
use buck2_directory::directory::builder_lazy::LazyDirectoryBuilder;
use buck2_directory::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_hasher::DirectoryDigester;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::directory_ref::DirectoryRef;
use buck2_directory::directory::directory_ref::FingerprintedDirectoryRef;
use buck2_directory::directory::directory_selector::DirectorySelector;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::find::DirectoryFindError;
use buck2_directory::directory::find::find;
use buck2_directory::directory::fingerprinted_directory::FingerprintedDirectory;
use buck2_directory::directory::immutable_directory::ImmutableDirectory;
use buck2_directory::directory::shared_directory::SharedDirectory;
use buck2_directory::directory::shared_directory::SharedDirectoryInternable;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_fs::paths::RelativePathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use ref_cast::RefCast;
use remote_execution as RE;

use crate::artifact_value::ArtifactValue;
use crate::digest::CasDigestFromReExt;
use crate::digest::CasDigestToReExt;
use crate::digest_config::DigestConfig;
use crate::materialize::materializer::CasDownloadInfo;
use crate::re::manager::ManagedRemoteExecutionClient;

#[allocative::root]
pub static INTERNER: LazyLock<DashMapDirectoryInterner<ActionDirectoryMember, TrackedFileDigest>> =
    LazyLock::new(DashMapDirectoryInterner::new);

impl SharedDirectoryInternable<TrackedFileDigest> for ActionDirectoryMember {
    fn interner() -> DashMapDirectoryInterner<Self, TrackedFileDigest> {
        INTERNER.dupe()
    }
}

#[derive(
    Clone,
    Debug,
    Dupe,
    PartialEq,
    Eq,
    Display,
    Allocative,
    Pagable,
    strong_hash::StrongHash
)]
pub enum ActionDirectoryMember {
    File(FileMetadata),
    Symlink(Arc<Symlink>),
    ExternalSymlink(Arc<ExternalSymlink>),
}

impl ActionDirectoryMember {
    pub fn size(&self) -> u64 {
        match self {
            ActionDirectoryMember::File(f) => f.digest.size(),
            ActionDirectoryMember::Symlink(_) => 0,
            ActionDirectoryMember::ExternalSymlink(_) => 0,
        }
    }
}

pub type ActionDirectoryEntry<D> = DirectoryEntry<D, ActionDirectoryMember>;

pub type ActionImmutableDirectory = ImmutableDirectory<ActionDirectoryMember, TrackedFileDigest>;

pub type ActionSharedDirectory = SharedDirectory<ActionDirectoryMember, TrackedFileDigest>;

// Interned directories exist in the millions; keep the node payload within the 64-byte jemalloc
// bin its Arc allocation occupies (4 words of data + 1 word interner handle + 2 words header).
buck2_util::size_assert::words_of_type!(
    buck2_directory::directory::shared_directory::SharedDirectoryData<
        ActionDirectoryMember,
        TrackedFileDigest,
    >,
    4
);

pub type ActionDirectoryBuilder = DirectoryBuilder<ActionDirectoryMember, TrackedFileDigest>;

pub type LazyActionDirectoryBuilder =
    LazyDirectoryBuilder<ActionDirectoryMember, TrackedFileDigest>;

pub trait ActionDirectory = Directory<ActionDirectoryMember, TrackedFileDigest>;

pub trait ActionDirectoryRef<'a> =
    DirectoryRef<'a, Leaf = ActionDirectoryMember, DirectoryDigest = TrackedFileDigest>;

pub trait ActionFingerprintedDirectory =
    FingerprintedDirectory<ActionDirectoryMember, TrackedFileDigest>;

pub trait ActionFingerprintedDirectoryRef<'a> = FingerprintedDirectoryRef<'a, Leaf = ActionDirectoryMember, DirectoryDigest = TrackedFileDigest>;

#[derive(Allocative, RefCast)]
#[repr(transparent)]
pub struct ReDirectorySerializer {
    pub cas_digest_config: CasDigestConfig,
}

impl ReDirectorySerializer {
    fn create_re_directory<'a, D, I>(
        entries: I,
        directory_renamer: Option<&dyn Fn(&FileName) -> Option<FileNameBuf>>,
    ) -> RE::Directory
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
        D: ActionFingerprintedDirectoryRef<'a>,
    {
        let mut files: Vec<RE::FileNode> = Vec::new();
        let mut directories: Vec<RE::DirectoryNode> = Vec::new();
        let mut symlinks: Vec<RE::SymlinkNode> = Vec::new();

        for (name, entry) in entries {
            match entry {
                DirectoryEntry::Dir(d) => {
                    let name = directory_renamer
                        .and_then(|f| f(name))
                        .unwrap_or_else(|| name.to_owned());
                    directories.push(RE::DirectoryNode {
                        name: name.as_str().to_owned(),
                        digest: Some(d.as_fingerprinted_dyn().fingerprint().to_grpc()),
                    });
                }
                // OSS vs internal divergence.
                #[allow(clippy::needless_update)]
                DirectoryEntry::Leaf(ActionDirectoryMember::File(f)) => {
                    files.push(RE::FileNode {
                        name: name.as_str().into(),
                        digest: Some(f.digest.to_grpc()),
                        is_executable: f.is_executable,
                        ..Default::default()
                    });
                }
                // OSS vs internal divergence.
                #[allow(clippy::needless_update)]
                DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(s)) => {
                    let target = if let Some(renamer) = directory_renamer {
                        let mut target = RelativePathBuf::with_capacity(s.target().as_str().len());
                        for comp in s.target().components() {
                            if let Some(renamed) = comp.as_normal().and_then(renamer) {
                                target.push(&renamed);
                            } else {
                                target.push(comp.as_relative_path());
                            }
                        }
                        target.as_str().to_owned()
                    } else {
                        s.to_string()
                    };
                    symlinks.push(RE::SymlinkNode {
                        name: name.as_str().into(),
                        target,
                        ..Default::default()
                    });
                }
                // OSS vs internal divergence.
                #[allow(clippy::needless_update)]
                DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(s)) => {
                    symlinks.push(RE::SymlinkNode {
                        name: name.as_str().into(),
                        target: s.target_str().to_owned(),
                        ..Default::default()
                    });
                }
            }
        }

        files.sort_by(|a, b| a.name.cmp(&b.name));
        directories.sort_by(|a, b| {
            a.name.cmp(&b.name).then(
                // This never happens in the "normal" case without a directory_renamer, but the
                // renamer breaks the standard CAS digesting scheme since we can now have two
                // directories with the same name but different digests. Therefore we use the
                // digest to get a deterministic ordering in those cases.
                a.digest
                    .as_ref()
                    .expect("By construction")
                    .hash
                    .cmp(&b.digest.as_ref().expect("By construction").hash),
            )
        });
        symlinks.sort_by(|a, b| a.name.cmp(&b.name));

        // OSS vs internal divergence.
        #[allow(clippy::needless_update)]
        RE::Directory {
            files,
            directories,
            symlinks,
            ..Default::default()
        }
    }

    pub fn serialize_entries<'a, D, I>(entries: I) -> Vec<u8>
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
        D: ActionFingerprintedDirectoryRef<'a>,
    {
        proto_serialize(&Self::create_re_directory(entries, None))
    }

    pub fn rename_and_serialize_entries<'a, D, I>(
        entries: I,
        directory_renamer: &dyn Fn(&FileName) -> Option<FileNameBuf>,
    ) -> Vec<u8>
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
        D: ActionFingerprintedDirectoryRef<'a>,
    {
        proto_serialize(&Self::create_re_directory(entries, Some(directory_renamer)))
    }
}

fn proto_serialize<M: prost::Message>(m: &M) -> Vec<u8> {
    m.encode_to_vec()
}

impl DirectoryDigester<ActionDirectoryMember, TrackedFileDigest> for ReDirectorySerializer {
    fn hash_entries<'a, D, I>(&self, entries: I) -> TrackedFileDigest
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
        D: ActionFingerprintedDirectoryRef<'a>,
    {
        TrackedFileDigest::from_content(&Self::serialize_entries(entries), self.cas_digest_config)
    }

    fn leaf_size(&self, leaf: &ActionDirectoryMember) -> u64 {
        leaf.size()
    }
}

pub fn new_symlink<T: AsRef<Path>>(target: T) -> buck2_error::Result<ActionDirectoryMember> {
    let target = target.as_ref();
    if target.is_absolute() {
        Ok(ActionDirectoryMember::ExternalSymlink(Arc::new(
            ExternalSymlink::new(target.to_path_buf(), ForwardRelativePathBuf::default())?,
        )))
    } else {
        Ok(ActionDirectoryMember::Symlink(Arc::new(Symlink::new(
            RelativePathBuf::from_system_path(target)?,
        ))))
    }
}

pub fn directory_to_re_tree<T>(directory: &T) -> RE::Tree
where
    T: ActionFingerprintedDirectory,
    for<'a> T::DirectoryRef<'a>: FingerprintedDirectoryRef<'a>,
{
    let children = directory
        .ordered_walk()
        .without_paths()
        .filter_map(|entry| match entry {
            DirectoryEntry::Dir(d) => Some(d),
            DirectoryEntry::Leaf(..) => None,
        })
        .map(|d| ReDirectorySerializer::create_re_directory(d.entries(), None))
        .collect();

    let root = ReDirectorySerializer::create_re_directory(directory.as_ref().entries(), None);

    RE::Tree {
        root: Some(root),
        children,
    }
}

pub async fn re_directory_to_re_tree(
    directory: RE::Directory,
    client: &ManagedRemoteExecutionClient,
    info: &CasDownloadInfo,
) -> buck2_error::Result<RE::Tree> {
    let mut children: Vec<RE::Directory> = vec![];
    let mut frontier = directory.directories.clone();
    while !frontier.is_empty() {
        let digests: Vec<remote_execution::TDigest> = frontier
            .into_iter()
            .filter_map(|d| d.digest)
            .map(|digest| remote_execution::TDigest {
                hash: digest.hash.clone(),
                size_in_bytes: digest.size_bytes,
                ..Default::default()
            })
            .collect();
        let mut retrieved = client
            .download_typed_blobs::<RE::Directory>(None, digests, info)
            .await?;
        frontier = retrieved
            .iter()
            .flat_map(|d| d.directories.clone())
            .collect();
        children.append(&mut retrieved);
    }
    Ok(RE::Tree {
        root: Some(directory),
        children,
    })
}

/// Constructs a `Directory` from an `RE::Tree`. As long as the
/// `RE::Tree` is valid (i.e. nothing is broken in the RE side), this
/// should always succeed.
#[allow(clippy::trivially_copy_pass_by_ref)] // SystemTime is a different size on Windows
pub fn re_tree_to_directory(
    tree: &RE::Tree,
    leaf_expires: &jiff::Timestamp,
    digest_config: DigestConfig,
    fingerprint: bool,
) -> buck2_error::Result<ActionDirectoryBuilder> {
    /// A map of digests to directories, populated lazily when we access it based on the hash we
    /// use. We need this because in a RE tree, the directories in the tree don't carry their hash,
    /// but the pointers are hashes, so we need to first see a hash before we can work out what
    /// hashing mechanism to use here.
    struct DirMap<'a> {
        by_kind: BuckMutInsertionMap<DigestAlgorithm, BuckMutMap<FileDigest, &'a RE::Directory>>,
        directories: &'a [RE::Directory],
    }

    impl<'a> DirMap<'a> {
        fn new(directories: &'a [RE::Directory]) -> Self {
            Self {
                by_kind: BuckMutInsertionMap::new(),
                directories,
            }
        }

        fn get(&mut self, algo: DigestAlgorithm, digest: &FileDigest) -> Option<&'a RE::Directory> {
            let map = self.by_kind.entry(algo).or_insert_with(|| {
                self.directories
                    .iter()
                    .map(|d| (from_proto_message(d, algo), d))
                    .collect()
            });

            map.get(digest).copied()
        }
    }

    fn from_proto_message<M: prost::Message>(m: &M, algo: DigestAlgorithm) -> FileDigest {
        let mut m_encoded = Vec::new();
        m.encode(&mut m_encoded)
            .unwrap_or_else(|e| unreachable!("Protobuf messages are always encodeable: {}", e));
        FileDigest::from_content_for_algorithm(m_encoded.as_slice(), algo)
    }

    // Recursively builds the directory
    fn dfs_build(
        re_dir: &'_ RE::Directory,
        re_dir_name: &'_ (impl fmt::Display + ?Sized),
        dirmap: &'_ mut DirMap<'_>,
        leaf_expires: &jiff::Timestamp,
        digest_config: DigestConfig,
        fingerprint: bool,
    ) -> buck2_error::Result<ActionDirectoryBuilder> {
        // A downloaded RE directory is a complete listing of its contents.
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        for node in &re_dir.files {
            let name = FileNameBuf::try_from(node.name.clone()).map_err(|_| {
                DirectoryReConversionError::IncorrectFileName {
                    name: node.name.clone(),
                    dir: re_dir_name.to_string(),
                }
            })?;

            let digest = node.digest.as_ref().ok_or_else(|| {
                DirectoryReConversionError::NodeWithDigestNone {
                    name: node.name.clone(),
                    dir: re_dir_name.to_string(),
                }
            })?;
            let digest = FileDigest::from_grpc(digest, digest_config)?;
            let digest = TrackedFileDigest::new_expires(
                digest,
                *leaf_expires,
                digest_config.cas_digest_config(),
            );

            let member = ActionDirectoryMember::File(FileMetadata {
                digest,
                is_executable: node.is_executable,
            });

            builder.insert(name, DirectoryEntry::Leaf(member))?;
        }
        for node in &re_dir.symlinks {
            builder.insert(
                FileNameBuf::try_from(node.name.clone()).map_err(|_| {
                    DirectoryReConversionError::IncorrectFileName {
                        name: node.name.clone(),
                        dir: re_dir_name.to_string(),
                    }
                })?,
                DirectoryEntry::Leaf(node.try_into()?),
            )?;
        }
        for dir_node in &re_dir.directories {
            let child_digest = match &dir_node.digest {
                None => {
                    return Err(DirectoryReConversionError::NodeWithDigestNone {
                        name: dir_node.name.clone(),
                        dir: re_dir_name.to_string(),
                    }
                    .into());
                }
                Some(d) => d,
            };
            let (child_digest, child_digest_algo) =
                FileDigest::from_grpc_with_algo(child_digest, digest_config)?;
            let child_re_dir = match dirmap.get(child_digest_algo, &child_digest) {
                None => {
                    return Err(DirectoryReConversionError::IncompleteTreeChildrenList {
                        name: dir_node.name.clone(),
                        dir: re_dir_name.to_string(),
                    }
                    .into());
                }
                Some(d) => d,
            };
            let dir = dfs_build(
                child_re_dir,
                &child_digest,
                dirmap,
                leaf_expires,
                digest_config,
                fingerprint,
            )?;
            builder.insert(
                FileNameBuf::try_from(dir_node.name.clone()).map_err(|_| {
                    DirectoryReConversionError::IncorrectFileName {
                        name: dir_node.name.clone(),
                        dir: re_dir_name.to_string(),
                    }
                })?,
                DirectoryEntry::Dir(dir),
            )?;
        }

        // NOTE: We re-digest the directories we just received here (instead of trusting RE with
        // the hashes). But, since output directories tend to be small, that doesn't actually
        // matter.
        // NOTE 2: We eagerly fingerprint since the only outputs we expect to
        // receive from RE are the ones we requested, so accordingly if we are
        // deserializing something here we expect to later turn it into an
        // output.
        if fingerprint {
            Ok(builder
                .fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER)
                .into_builder())
        } else {
            Ok(builder)
        }
    }

    let root_dir = match &tree.root {
        Some(d) => d,
        None => return Ok(ActionDirectoryBuilder::empty_exhaustive()),
    };

    dfs_build(
        root_dir,
        "root directory",
        &mut DirMap::new(&tree.children),
        leaf_expires,
        digest_config,
        fingerprint,
    )
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum DirectoryReConversionError {
    // Conversion from RE::Tree errors (these shouldn't happen unless something is broken on RE side)
    #[error("Converting RE::Tree to Directory, dir `{dir}` has child `{name}` with digest=None.")]
    NodeWithDigestNone { name: String, dir: String },
    #[error(
        "Converting RE::Tree to Directory, dir `{dir}` has child `{name}` not in tree.children."
    )]
    IncompleteTreeChildrenList { name: String, dir: String },
    #[error("Converting RE::Tree to Directory, dir `{dir}` has incorrect file name `{name}`")]
    IncorrectFileName { name: String, dir: String },
}

impl<'a> TryFrom<&'a RE::SymlinkNode> for ActionDirectoryMember {
    type Error = buck2_error::Error;

    fn try_from(node: &'a RE::SymlinkNode) -> Result<Self, Self::Error> {
        let symlink = if node.target.starts_with('/') {
            ActionDirectoryMember::ExternalSymlink(Arc::new(ExternalSymlink::new(
                PathBuf::from(node.target.as_str()),
                ForwardRelativePathBuf::default(),
            )?))
        } else {
            ActionDirectoryMember::Symlink(Arc::new(Symlink::new(RelativePathBuf::from(
                node.target.clone(),
            ))))
        };
        Ok(symlink)
    }
}

pub fn relativize_directory(
    builder: &mut ActionDirectoryBuilder,
    orig_root: &ProjectRelativePath,
    new_root: &ProjectRelativePath,
) -> buck2_error::Result<()> {
    let mut replacements = ActionDirectoryBuilder::empty_non_exhaustive();

    {
        let mut walk = builder.unordered_walk_leaves();
        while let Some((path, entry)) = walk.next() {
            let link = match entry {
                ActionDirectoryMember::Symlink(link) => link,
                _ => continue,
            };

            let path = path.get();

            let orig_path = orig_root.join(&path);
            let new_path = new_root.join(&path);

            let orig_dest = orig_path
                .parent()
                .internal_error("Symlink has no dir parent")?
                .join_normalized(link.target())?;

            let new_dest = new_path
                .parent()
                .internal_error("Symlink has no dir parent")?
                .as_forward_relative_path()
                .as_relative_path()
                .relative(orig_dest);
            let new_link = Arc::new(Symlink::new(new_dest));

            replacements.insert(
                &path,
                DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(new_link)),
            )?;
        }
    }

    builder.merge(replacements)?;

    Ok(())
}

pub fn override_executable_bit(
    builder: &mut ActionDirectoryBuilder,
    executable_bit_override: bool,
) -> buck2_error::Result<()> {
    let mut replacements = ActionDirectoryBuilder::empty_non_exhaustive();

    {
        let mut walk = builder.unordered_walk_leaves();
        while let Some((path, entry)) = walk.next() {
            let file = match entry {
                ActionDirectoryMember::File(file) => {
                    file.dupe().with_executable(executable_bit_override)
                }
                _ => continue,
            };

            let path = path.get();

            replacements.insert(
                &path,
                DirectoryEntry::Leaf(ActionDirectoryMember::File(file)),
            )?;
        }
    }

    builder.merge(replacements)?;

    Ok(())
}

pub fn insert_entry<D>(
    builder: &mut impl DirectoryBuilderLike<D, ActionDirectoryMember>,
    path: ProjectRelativePathBuf,
    entry: ActionDirectoryEntry<D>,
) -> buck2_error::Result<()> {
    if let DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(s)) = entry {
        // ExternalSymlink is a bit of an odd concept. The way it works is that when you try to
        // read `foo/bar` and it turns out `foo` is an ExternalSymlink, we tell you the value of
        // `foo/bar` is an `ExternalSymlink` to some location with the "rest of the path" being
        // `bar`.

        // Later, when you try to add this to an action directory (this code), we need to put a
        // symlink at `foo` but all we have is the path this `ExternalSymlink` piece of data exists
        // at (`foo/bar`), and the "rest" of the path (`bar`), so we strip the rest of the path and
        // conclude the symlink must be at `foo` and should point at the external symlink's path
        // without the "rest" component.

        // This is kinda wonky though it sort of works. However, it does not work when we try to
        // read a symlink that points through an external symlink, e.g. reading `bar` in those
        // circumstances:

        // ```
        // bar -> foo/xxx
        // foo -> /home/blah
        // ```

        // When this happens, the "value" of `bar` is `ExternalSymlink(..., rest = "xxx")`, but the
        // path is `bar`, and we can't infer that the path must actually be `bar` and that the
        // symlink should be `/home/blah/xxx` (so in this case we keep the rest).

        // NOTE (torozco): I think we *could* perhaps make this work by using `path` as-is if we
        // can't "fix" it, and using the full symlink destination instead of
        // `s.without_remaining_path()`, but considering this doesn't seem to be something very
        // widespread and I might be missing something, I did not do it.
        let fixed_source_path = s.fix_source_path(path.as_ref()).ok_or_else(|| {
            internal_error!("Error locating source path for symlink at {path}: {s}")
        })?;
        let path = ProjectRelativePath::unchecked_new(fixed_source_path.as_str()).to_buf();
        let entry = DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(
            s.without_remaining_path(),
        ));
        builder.insert(path.into(), entry)?;
    } else {
        builder.insert(path.into(), entry)?;
    }

    Ok(())
}

/// Inserts an input to the tree, which will be required when following
/// symlinks to calculate the `deps` of the `ArtifactValue`.
pub fn insert_artifact(
    builder: &mut ActionDirectoryBuilder,
    path: ProjectRelativePathBuf,
    value: &ArtifactValue,
) -> buck2_error::Result<()> {
    insert_entry(
        builder,
        path,
        value.entry().dupe().map_dir(|d| d.into_builder()),
    )?;
    // add input's deps
    if let Some(deps) = value.deps() {
        builder.merge(deps.dupe().into_builder())?;
    }
    Ok(())
}

pub fn insert_artifact_lazy(
    builder: &mut LazyActionDirectoryBuilder,
    path: ProjectRelativePathBuf,
    value: &ArtifactValue,
) -> buck2_error::Result<()> {
    insert_entry(builder, path, value.entry().dupe())?;
    // add input's deps
    if let Some(deps) = value.deps() {
        builder.merge(deps.dupe())?;
    }
    Ok(())
}

pub fn insert_file<D>(
    builder: &mut impl DirectoryBuilderLike<D, ActionDirectoryMember>,
    path: ProjectRelativePathBuf,
    meta: FileMetadata,
) -> buck2_error::Result<()> {
    insert_entry(
        builder,
        path,
        DirectoryEntry::Leaf(ActionDirectoryMember::File(meta)),
    )
}

#[cfg(test)]
pub fn insert_symlink(
    builder: &mut ActionDirectoryBuilder,
    path: ProjectRelativePathBuf,
    symlink: Arc<Symlink>,
) -> buck2_error::Result<()> {
    insert_entry(
        builder,
        path,
        DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(symlink)),
    )
}

pub fn expand_selector_for_dependencies(
    builder: &ActionDirectoryBuilder,
    paths_to_take: &mut DirectorySelector,
) {
    // Note that we receive the list of paths to take as an input and start from that for the paths
    // to visit. Rhis ensures that if we add symlinks that point into the paths we are about to
    // take out, we don't bother visiting them individually, since we'll start by visiting the whole
    // thing.
    let mut paths_to_visit = paths_to_take.clone();

    let mut all_known_symlinks = BuckMutSet::default();

    while !paths_to_visit.is_empty() {
        let mut next_paths_to_visit = DirectorySelector::empty();

        let mut handle_link_at_path = |link_path: &ForwardRelativePath, link: &Symlink| {
            let link_dst = match link_path.parent() {
                Some(parent) => parent.join_system_normalized(&link.target().to_path("")),
                None => ForwardRelativePath::new(link.target().as_str()).map(ToOwned::to_owned),
            };

            let dest = match link_dst {
                Ok(d) => d,
                Err(e) => {
                    // If we get here the symlink we found points to something that is not
                    // relative to this directory so we bail.
                    tracing::trace!(
                        "Invalid symlink: {} -> {}: {:#}",
                        link_path,
                        link.target(),
                        e
                    );
                    if cfg!(test) {
                        panic!("Invalid symlink found")
                    }
                    return;
                }
            };

            if all_known_symlinks.insert(dest.clone()) {
                tracing::trace!("New symlink: {} -> {}", link_path, dest);
                next_paths_to_visit.select(&dest);
                paths_to_take.select(&dest);
            }
        };

        let mut search = paths_to_visit.unordered_search(builder);

        while let Some((entry_path, entry)) = search.next() {
            // We're searching for all paths that are symlink destinations to find _new_ symlinks.
            // We might encounter paths that don't actually work while we do this. If that happens,
            // then we might be in a situation where we were trying to traverse *through* a
            // symlink. We check for that and handle the newly-discovered symlink if that happens.
            let mut entry_walk = match entry {
                Ok(entry) => unordered_entry_walk(entry),
                Err(e) => {
                    let link = match e.into_leaf() {
                        ActionDirectoryMember::Symlink(link) => link,
                        e => {
                            tracing::trace!(
                                "Ignoring entry to visit at: {}: cannot traverse through: {:?}",
                                entry_path,
                                e,
                            );

                            continue;
                        }
                    };

                    handle_link_at_path(&entry_path.get(), link);
                    continue;
                }
            };

            while let Some((sub_entry_path, sub_entry)) = entry_walk.next() {
                let link = match sub_entry {
                    DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(link)) => link,
                    _ => continue,
                };

                let link_path = entry_path.get().join(sub_entry_path.get());
                handle_link_at_path(&link_path, link);
            }
        }

        paths_to_visit = next_paths_to_visit;
    }
}

/// Given a builder and a Path, return it as an ArtifactValue. Dependencies are produced by
/// extracting any symlinks found in the builder.
pub fn extract_artifact_value(
    builder: &ActionDirectoryBuilder,
    path: &ProjectRelativePath,
    digest_config: DigestConfig,
) -> buck2_error::Result<Option<ArtifactValue>> {
    let entry = match find(builder.as_ref(), path.as_forward_relative_path())? {
        Some(entry) => entry,
        _ => return Ok(None),
    };

    // This will be the set of all paths referenced by symlinks in our directory.
    let mut paths_to_take = DirectorySelector::empty();
    paths_to_take.select(path);

    expand_selector_for_dependencies(builder, &mut paths_to_take);

    let mut deps = ActionDirectoryBuilder::empty_non_exhaustive();
    let mut has_deps = false;

    for (entry_path, entry) in paths_to_take.unordered_search(builder).with_paths() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                // We'll get here if we have a symlink that points at a path that traverses another
                // symlink (the my_genrule10 testcase has an example of this).
                tracing::trace!(
                    "Taking entry as a symlink traverses through it: {}",
                    entry_path
                );
                DirectoryEntry::Leaf(err.into_leaf())
            }
        };

        if entry_path == path.as_forward_relative_path() {
            continue;
        }

        has_deps = true;

        // TODO (@torozco): We could improve this a lot by sharing() those entries in place when we
        // extract them.
        deps.insert(
            &entry_path,
            entry.map_leaf(|l| l.dupe()).map_dir(|d| d.to_builder()),
        )?;
    }

    let entry = entry.map_leaf(|l| l.dupe()).map_dir(|d| {
        d.to_builder()
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER)
    });

    // Do not depend on the artifact itself.
    match deps.remove_prefix(path.as_forward_relative_path()) {
        Ok(_) => {}
        Err(DirectoryFindError::CannotTraverseLeaf { .. }) => {
            return Err(internal_error!(
                "Dependency artifact is parent of output artifact: {}",
                path
            ));
        }
    }

    let deps = if has_deps {
        Some(
            deps.fingerprint(digest_config.as_directory_serializer())
                .shared(&*INTERNER),
        )
    } else {
        None
    };

    Ok(Some(ArtifactValue::new(entry, deps)))
}

#[cfg(test)]
mod tests {

    use super::*;

    /// A local output symlink whose target is not a relative path buck2 can represent is an error, not
    /// a panic. On Windows `\` is a path separator, so only Unix has such a target.
    #[cfg(unix)]
    #[test]
    fn test_new_symlink_target_with_backslash() {
        assert!(new_symlink(r"a\b").is_err());
        assert!(new_symlink("a/b").is_ok());
    }

    fn path(s: &str) -> ProjectRelativePathBuf {
        ProjectRelativePath::new(s).unwrap().to_buf()
    }

    fn assert_dirs_eq(d1: &impl ActionDirectory, d2: &impl ActionDirectory) {
        let mut w1 = d1.ordered_walk();
        let mut w2 = d2.ordered_walk();

        loop {
            let n1 = w1.next();
            let n2 = w2.next();

            let ((p1, e1), (p2, e2)) = match (n1, n2) {
                (Some(n1), Some(n2)) => (n1, n2),
                (Some((p1, _)), None) => {
                    panic!("Walks differ, first difference at: {p1}");
                }
                (None, Some((p2, _))) => {
                    panic!("Walks differ, first difference at: {p2}");
                }
                (None, None) => break,
            };

            // Compare paths
            assert_eq!(p1.get(), p2.get(), "Walks produced different paths");

            // Compare leaves
            assert_eq!(
                e1.map_dir(|_| ()),
                e2.map_dir(|_| ()),
                "Walks differ at path {p1}"
            );
        }
    }

    #[test]
    fn directory_relativized() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut dir = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            insert_file(
                &mut builder,
                path("inter/f"),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            insert_symlink(
                &mut builder,
                path("sym"),
                Arc::new(Symlink::new("inter/f".into())),
            )?;
            insert_symlink(
                &mut builder,
                path("esym"),
                Arc::new(Symlink::new("../f".into())),
            )?;
            builder
        };

        let expected_dir = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            insert_file(
                &mut builder,
                path("inter/f"),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            insert_symlink(
                &mut builder,
                path("sym"),
                Arc::new(Symlink::new("../a/d0/inter/f".into())),
            )?;
            insert_symlink(
                &mut builder,
                path("esym"),
                Arc::new(Symlink::new("../a/f".into())),
            )?;
            builder
        };

        // Move directory from a/d0 to b.
        relativize_directory(&mut dir, &path("a/d0"), &path("b"))?;

        assert_dirs_eq(&dir, &expected_dir);

        Ok(())
    }

    fn build_test_dir() -> buck2_error::Result<ActionDirectoryBuilder> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        // /
        // |-f1
        // |-d1/
        // | |-d2/
        // | | |-d3/
        // | | | |-s1 -> ../../../d6/s4          d3_deps:[s4, f4]
        // | | | |-s2 -> ../d4                   d3_deps:[d4/*]
        // | | | |-s3 -> ../d5/../../../f1       d3_deps:[f1]
        // | | |-d4/
        // | | | |-f2
        // | | |-d5/
        // | | | |-f3
        // |-d6/
        // | |-s4 -> f4
        // | |-f4
        // | |-e1 -> /mnt/gvfs
        // |-f5

        for file in &["f1", "d1/d2/d4/f2", "d1/d2/d5/f3", "d6/f4", "f5"] {
            insert_file(
                &mut builder,
                path(file),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
        }

        for (sym, target) in &[
            ("d1/d2/d3/s1", "../../../d6/s4"),
            ("d1/d2/d3/s2", "../d4"),
            ("d1/d2/d3/s3", "../d5/../../../f1"),
            ("d6/s4", "f4"),
            ("d6/e1", "/mnt/gvfs"),
        ] {
            insert_symlink(
                &mut builder,
                path(sym),
                Arc::new(Symlink::new(target.into())),
            )?;
        }

        // Real action trees hold artifact content, which is exhaustive.
        builder.mark_uniformly_exhaustive();

        Ok(builder)
    }

    /// All deps of d6 are internal to it, so we don't return any deps.
    #[test]
    fn test_extract_no_deps() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();
        let root = build_test_dir()?;
        let value = extract_artifact_value(&root, &path("d6"), digest_config)?
            .internal_error("Not value!")?;
        assert!(value.deps().is_none());
        Ok(())
    }

    #[test]
    fn test_extract_has_deps_dir() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let root = build_test_dir()?;
        let value = extract_artifact_value(&root, &path("d1/d2/d3"), digest_config)?
            .internal_error("Not value!")?;

        let expected = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

            for p in &["d6/s4", "d6/f4", "d1/d2/d4", "f1"] {
                let path = path(p);
                let entry = find(root.as_ref(), path.as_forward_relative_path())?
                    .with_internal_error(|| format!("Missing {path}"))?
                    .map_dir(|d| d.to_builder())
                    .map_leaf(|l| l.dupe());
                insert_entry(&mut builder, path, entry)?;
            }

            builder
        };

        assert_dirs_eq(value.deps().internal_error("No deps!")?, &expected);

        Ok(())
    }

    /// The extracted value's boundary structure: the entry is complete content; the deps tree
    /// is scaffolding whose exhaustive subtrees are exactly the captured dep regions.
    #[test]
    fn test_extract_deps_marking() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let root = build_test_dir()?;
        let value = extract_artifact_value(&root, &path("d1/d2/d3"), digest_config)?
            .internal_error("Not value!")?;

        match value.entry() {
            DirectoryEntry::Dir(d) => assert!(d.exhaustiveness_hash().is_uniformly_exhaustive()),
            _ => panic!("Expected a dir entry"),
        }

        let deps = value.deps().internal_error("No deps!")?;
        assert!(!deps.exhaustiveness_hash().is_exhaustive());

        let region = find(deps.as_ref(), ForwardRelativePath::new("d1/d2/d4").unwrap())?
            .internal_error("Missing region")?;
        match region {
            DirectoryEntry::Dir(d) => assert!(d.exhaustiveness_hash().is_uniformly_exhaustive()),
            _ => panic!("Expected a dir region"),
        }

        Ok(())
    }

    #[test]
    fn test_extract_has_deps_leaf() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let root = build_test_dir()?;
        let value = extract_artifact_value(&root, &path("d1/d2/d3/s3"), digest_config)?
            .internal_error("Not value!")?;

        let expected = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            insert_file(
                &mut builder,
                path("f1"),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
            builder
        };

        assert_dirs_eq(value.deps().internal_error("No deps!")?, &expected);

        Ok(())
    }

    #[test]
    fn test_extract_cycle() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        for (sym, target) in &[("d1/f1", "../d2/f2"), ("d2/f2", "../d1/f1")] {
            insert_symlink(
                &mut builder,
                path(sym),
                Arc::new(Symlink::new(target.into())),
            )?;
        }

        let expected = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
            insert_symlink(
                &mut builder,
                path("d2/f2"),
                Arc::new(Symlink::new("../d1/f1".into())),
            )?;
            builder
        };

        let value = extract_artifact_value(&builder, &path("d1/f1"), digest_config)?
            .internal_error("Not value!")?;

        assert_dirs_eq(value.deps().internal_error("No deps!")?, &expected);

        Ok(())
    }

    #[test]
    fn test_extract_symlink_chain() -> buck2_error::Result<()> {
        // Crank up the difficulty: l1 points through d3/f, but through l2. We need all of those in
        // the deps! In practice, this tends to not happen in Buck 2 because we always dereference
        // symlinks and traverse them, but might a well support it properly.
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_symlink(
            &mut builder,
            path("l1"),
            Arc::new(Symlink::new("l2/f".into())),
        )?;

        insert_symlink(
            &mut builder,
            path("l2"),
            Arc::new(Symlink::new("d3".into())),
        )?;

        insert_file(
            &mut builder,
            path("d3/f"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;

        let value = extract_artifact_value(&builder, &path("l1"), digest_config)?
            .internal_error("Not value!")?;

        let expected = {
            let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

            insert_symlink(
                &mut builder,
                path("l2"),
                Arc::new(Symlink::new("d3".into())),
            )?;

            insert_file(
                &mut builder,
                path("d3/f"),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;

            builder
        };

        assert_dirs_eq(value.deps().internal_error("No deps!")?, &expected);

        Ok(())
    }

    #[test]
    fn test_re_tree_roundtrip() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_file(
            &mut builder,
            path("a/aa"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_file(
            &mut builder,
            path("a/bb"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_file(
            &mut builder,
            path("b/b"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        let dir = builder.fingerprint(digest_config.as_directory_serializer());

        let tree = directory_to_re_tree(&dir);
        let dir2 = re_tree_to_directory(&tree, &jiff::Timestamp::now(), digest_config, true)?;

        assert_dirs_eq(&dir, &dir2);

        Ok(())
    }

    #[test]
    fn test_symlink_to_current_directory_round_trips() -> buck2_error::Result<()> {
        // A directory with normalized `.` targets must retain writable symlink targets after
        // serialization to an RE tree and deserialization back into an action directory.
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_file(
            &mut builder,
            path("out/x"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        for (link, target) in [
            ("out/here", "."),
            ("out/dotslash", "./"),
            ("out/d/here", "../x"),
        ] {
            let target = RelativePathBuf::from_system_path(Path::new(target))?;
            insert_symlink(&mut builder, path(link), Arc::new(Symlink::new(target)))?;
        }

        let target = |link: &str| match find(
            Directory::as_ref(&builder),
            ForwardRelativePath::new(link)?,
        )? {
            Some(DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(s))) => {
                buck2_error::Ok(s.target().as_str().to_owned())
            }
            _ => panic!("`{link}` is not a symlink"),
        };
        assert_eq!(target("out/here")?, ".");
        assert_eq!(target("out/dotslash")?, ".");
        assert_eq!(target("out/d/here")?, "../x");

        // Following the link does not leave the directory.
        let value = extract_artifact_value(&builder, &path("out/here"), digest_config)?
            .internal_error("Not value!")?;
        assert!(matches!(
            value.entry(),
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(_))
        ));

        let dir = builder.fingerprint(digest_config.as_directory_serializer());
        let tree = directory_to_re_tree(&dir);
        for node in tree.root.as_ref().unwrap().symlinks.iter() {
            assert!(
                !node.target.is_empty(),
                "`{}` has an empty target",
                node.name
            );
        }
        let dir2 = re_tree_to_directory(&tree, &jiff::Timestamp::now(), digest_config, true)?;
        assert_dirs_eq(&dir, &dir2);

        Ok(())
    }

    /// Ensure that we serialize trees the same way RE does. The expected hash was obtained by
    /// running:
    ///
    /// ```sh
    /// buck2 run fbcode//remote_execution/rust/recli:recli -- \
    ///     exec command --out-dir test -- sh -c \
    ///     'mkdir -p test/a/aa test/a/aaa test/b/bb test/d && touch test/a/aa/f test/a/aaa/f test/b/bb/f test/d/f'
    /// ```
    #[test]
    fn test_re_tree_compatibility() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        for p in &["a/aa/f", "a/aaa/f", "b/bb/f", "d/f"] {
            insert_file(
                &mut builder,
                path(p),
                FileMetadata::empty(digest_config.cas_digest_config()),
            )?;
        }
        let dir = builder.fingerprint(digest_config.as_directory_serializer());
        let tree = directory_to_re_tree(&dir);
        let tree = proto_serialize(&tree);
        let digest = FileDigest::from_content(&tree, digest_config.cas_digest_config());
        assert_eq!(
            digest.to_string(),
            "28e2632abbdc186ddd3ce26d2faf2da40c4c2695:521"
        );

        Ok(())
    }

    #[test]
    // Test that a `Symlink` (relative-target) created with a Windows path doesn't get
    // interpreted as an invalid symlink. The `ExternalSymlink` (absolute-target) case
    // is covered separately by `external_symlink_serializer_*` above and by
    // `normalize_target_for_re_*` in `buck2_common::external_symlink`, which together
    // ensure both arms store POSIX-style targets when uploaded to CAS.
    fn test_unnormalized_symlinks() -> buck2_error::Result<()> {
        if !cfg!(windows) {
            return Ok(());
        }

        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_symlink(
            &mut builder,
            path("d1/f1"),
            Arc::new(Symlink::new("..\\d2\\f2".into())),
        )?;
        insert_file(
            &mut builder,
            path("d2/f2"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;

        extract_artifact_value(&builder, &path("d1/f1"), digest_config)?
            .internal_error("Not value!")?;
        Ok(())
    }

    /// Pins the contract that `ReDirectorySerializer::create_re_directory`
    /// is a passthrough for `ExternalSymlink::target_str()`: whatever string
    /// is stored on the symlink ends up verbatim in `RE::SymlinkNode.target`
    /// on the wire. This is the load-bearing assumption behind fixing the
    /// Windows→Linux RE backslash bug at the `ExternalSymlink` constructor
    /// rather than at the serializer — if this test ever fails, the fix
    /// site must be re-evaluated.
    #[test]
    fn external_symlink_serializer_preserves_target_str_verbatim() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        let sym = Arc::new(ExternalSymlink::new(
            PathBuf::from("/mnt/gvfs/openssl/lib"),
            ForwardRelativePathBuf::default(),
        )?);
        insert_entry(
            &mut builder,
            path("openssl"),
            DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(sym.dupe())),
        )?;

        let dir = builder.fingerprint(digest_config.as_directory_serializer());
        let re_dir = ReDirectorySerializer::create_re_directory(dir.as_ref().entries(), None);

        assert_eq!(re_dir.symlinks.len(), 1, "Expected exactly one symlink");
        assert_eq!(re_dir.symlinks[0].name, "openssl");
        assert_eq!(
            re_dir.symlinks[0].target,
            sym.target_str(),
            "Serializer must round-trip target_str verbatim",
        );
        assert_eq!(re_dir.symlinks[0].target, "/mnt/gvfs/openssl/lib");
        Ok(())
    }

    /// Defense-in-depth round-trip: an `ExternalSymlink` constructed with a
    /// backslash-bearing target on a non-Windows host (the `cfg!(windows)`
    /// gate is false, so the constructor's normalizer is a no-op) must
    /// produce the SAME backslashed string in `RE::SymlinkNode.target`.
    /// Together with the constructor's unit tests this proves the bug
    /// surfaces *only* at the constructor site — nothing downstream
    /// reintroduces or hides the problem. On Windows the constructor would
    /// have normalized away the backslashes before reaching the serializer.
    #[cfg(not(windows))]
    #[test]
    fn external_symlink_serializer_does_not_hide_backslashes_on_non_windows()
    -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
        let sym = Arc::new(ExternalSymlink::new(
            PathBuf::from(r"\mnt\gvfs\openssl\lib"),
            ForwardRelativePathBuf::default(),
        )?);
        // Sanity-check the precondition: on non-Windows the constructor is
        // a no-op so the stored target retains its backslashes.
        assert_eq!(sym.target_str(), r"\mnt\gvfs\openssl\lib");

        insert_entry(
            &mut builder,
            path("openssl"),
            DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(sym.dupe())),
        )?;

        let dir = builder.fingerprint(digest_config.as_directory_serializer());
        let re_dir = ReDirectorySerializer::create_re_directory(dir.as_ref().entries(), None);

        assert_eq!(re_dir.symlinks.len(), 1);
        assert_eq!(
            re_dir.symlinks[0].target, r"\mnt\gvfs\openssl\lib",
            "Serializer is a passthrough; the only fix site is the constructor",
        );
        Ok(())
    }

    struct TestSerializer {
        pub cas_digest_config: CasDigestConfig,
    }

    impl DirectoryDigester<ActionDirectoryMember, TrackedFileDigest> for TestSerializer {
        fn hash_entries<'a, D, I>(&self, entries: I) -> TrackedFileDigest
        where
            I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a ActionDirectoryMember>)>,
            D: ActionFingerprintedDirectoryRef<'a>,
        {
            fn rename(file_name: &FileName) -> Option<FileNameBuf> {
                if file_name.as_str() == "a" || file_name.as_str() == "b" {
                    Some(FileNameBuf::unchecked_new("replaced"))
                } else {
                    None
                }
            }
            TrackedFileDigest::from_content(
                &ReDirectorySerializer::rename_and_serialize_entries(entries, &rename),
                self.cas_digest_config,
            )
        }

        fn leaf_size(&self, leaf: &ActionDirectoryMember) -> u64 {
            match leaf {
                ActionDirectoryMember::File(f) => f.digest.size(),
                ActionDirectoryMember::Symlink(_) => 0,
                ActionDirectoryMember::ExternalSymlink(_) => 0,
            }
        }
    }

    #[test]
    fn test_rename_and_serialize() -> buck2_error::Result<()> {
        let digest_config = DigestConfig::testing_default();

        let mut builder1 = ActionDirectoryBuilder::empty_non_exhaustive();

        insert_file(
            &mut builder1,
            path("a/aa"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_file(
            &mut builder1,
            path("b/bb"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;

        let mut builder2 = ActionDirectoryBuilder::empty_non_exhaustive();
        insert_file(
            &mut builder2,
            path("b/aa"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;
        insert_file(
            &mut builder2,
            path("a/bb"),
            FileMetadata::empty(digest_config.cas_digest_config()),
        )?;

        let non_renamed_fingerprint1 = builder1
            .clone()
            .fingerprint(digest_config.as_directory_serializer());
        let non_renamed_fingerprint2 = builder2
            .clone()
            .fingerprint(digest_config.as_directory_serializer());

        assert!(non_renamed_fingerprint1 != non_renamed_fingerprint2);

        let renamed_fingerprint1 = builder1.fingerprint(&TestSerializer {
            cas_digest_config: digest_config.cas_digest_config(),
        });
        let renamed_fingerprint2 = builder2.fingerprint(&TestSerializer {
            cas_digest_config: digest_config.cas_digest_config(),
        });

        assert!(renamed_fingerprint1 == renamed_fingerprint2);

        Ok(())
    }
}
