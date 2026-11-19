/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use buck2_cli_proto::BuildTarget;
use buck2_cli_proto::build_target::BuildOutput;
use buck2_client_ctx::exit_result::ClientIoError;
use buck2_client_ctx::output_destination_arg::OutputDestinationArg;
use buck2_core::fs::project::ProjectRoot;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::working_dir::AbsWorkingDir;
use futures::StreamExt;
use futures::TryStreamExt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

const COPY_DIRECTORY_CONCURRENCY: usize = 4;

#[derive(Clone)]
struct CopyContext {
    // Any symlink pointing outside of this path would be copied/recreated as an absolute symlink to original target.
    // Should be canonicalized.
    relative_symlink_boundary: AbsNormPathBuf,
}

enum CopyWork {
    Directory {
        destination: AbsPathBuf,
        ready: oneshot::Sender<()>,
    },
    File {
        source: AbsPathBuf,
        destination: AbsPathBuf,
    },
    Symlink {
        source: AbsPathBuf,
        destination: AbsPathBuf,
    },
}

enum CopyQueueItem {
    Work(CopyWork),
    Error(buck2_error::Error),
}

/// Given a list of targets built by this command, extracts a reasonable default output from the list and writes it
/// to the path given by `out`.
///
/// In order to extract a "reasonable default output", this function will bail if any of the following are true:
///  1. Multiple top-level targets were built, in which case the correct output to write is ambiguous,
///  2. A single top-level target was built, but it produced zero default outputs,
///  3. A single top-level target was built, but it produced more than two default outputs
///
/// Otherwise, we'll extract the single default output from the single top-level target and copy it to the output
/// path. If the given path is a directory then all output files will be copied inside of it.
///
/// Symbolic links are preserved. However, if a relative symlink within one of the outputs points outside that output,
/// it will be converted into an absolute link pointing to the same target as the original link.
/// On Windows platform relative symlinks are always converted to an absolute one, though the same rule applies to whether a copied
/// link would point to the same original target or a copied one.
///
/// As a special case, `--out -` is interpreted as `--out /dev/stdout` and allows multiple output files to be
/// written to it.
pub(super) async fn copy_to_out(
    targets: &[BuildTarget],
    root_path: &ProjectRoot,
    working_dir: &AbsWorkingDir,
    out: &OutputDestinationArg,
) -> buck2_error::Result<()> {
    struct OutputToBeCopied {
        from_path: AbsNormPathBuf,
        is_dir: bool,
    }

    let mut outputs_to_be_copied = Vec::new();
    for target in targets {
        let default_outputs: Vec<&BuildOutput> = target
            .outputs
            .iter()
            .filter(|output| {
                output
                    .providers
                    .as_ref()
                    .is_none_or(|p| p.default_info && !p.other)
            })
            .collect();

        let single_default_output = match default_outputs.len() {
            0 => {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CopyOutputs,
                    "target {} produced zero default outputs",
                    target.target
                ));
            }
            1 => &default_outputs[0],
            n => {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CopyOutputs,
                    "target {} produced {} outputs, choice of output is ambiguous",
                    target.target,
                    n
                ));
            }
        };

        let output_path = root_path
            .root()
            .join(ForwardRelativePath::new(&single_default_output.path)?);
        let output_meta = tokio::fs::metadata(&output_path)
            .await
            .buck_error_context("Error inspecting file metadata")?;
        let is_dir = output_meta.is_dir();

        outputs_to_be_copied.push(OutputToBeCopied {
            from_path: output_path,
            is_dir,
        });
    }

    match out {
        OutputDestinationArg::Stream => {
            // Check no output is a directory. We allow outputting any number of
            // files (including 0) to stdout.
            if let Some(dir_i) = outputs_to_be_copied.iter().position(|o| o.is_dir) {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CopyOutputs,
                    "target {} produces a default output that is a directory, and cannot be sent to stdout",
                    targets[dir_i].target,
                ));
            }
        }
        OutputDestinationArg::Path(..) => {
            // Check we are outputting exactly 1 target. Okay if directory.
            if outputs_to_be_copied.len() != 1 {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::CopyOutputs,
                    "build command built multiple top-level targets, choice of output is ambiguous"
                ));
            }
        }
    }

    for to_be_copied in outputs_to_be_copied {
        match out {
            OutputDestinationArg::Stream => {
                let mut file = async_fs_util::open(&to_be_copied.from_path)
                    .await
                    .categorize_tagged(ErrorTag::CopyOutputs)?;
                tokio::io::copy(&mut file, &mut tokio::io::stdout())
                    .await
                    .map_err(convert_broken_pipe_error)?;
            }
            OutputDestinationArg::Path(path) => {
                let path = path.resolve(working_dir);
                if to_be_copied.is_dir {
                    let context = CopyContext {
                        relative_symlink_boundary: fs_util::canonicalize(&to_be_copied.from_path)
                            .categorize_tagged(ErrorTag::CopyOutputs)?,
                    };
                    copy_directory(&to_be_copied.from_path, &path, &context).await?;
                } else {
                    copy_file(&to_be_copied.from_path, &path).await?;
                }
            }
        }
    }

    Ok(())
}

fn copy_symlink<P: AsRef<AbsPath>, Q: AsRef<AbsPath>>(
    src_path: P,
    dst_path: Q,
    context: &CopyContext,
) -> buck2_error::Result<()> {
    // Make symlinks overwrite items which were already present at destination path
    fs_util::remove_all(&dst_path)
        .categorize_tagged(ErrorTag::CopyOutputs)
        .buck_error_context(format!(
            "Removing pre-existing item at path {:?}",
            src_path.as_ref()
        ))?;
    let symlink_target_abs_path = fs_util::canonicalize(src_path.as_ref())
        .categorize_tagged(ErrorTag::CopyOutputs)
        .buck_error_context(format!(
            "Resolving symlink to be copied {:?}",
            src_path.as_ref()
        ))?;
    // Now recreate the symlink
    let symlink_target = {
        if symlink_target_abs_path.starts_with(&context.relative_symlink_boundary) {
            // Symlink is not pointing outside the original output we are copying.
            // Just keep it as it is.
            fs_util::read_link(&src_path)
                .categorize_tagged(ErrorTag::CopyOutputs)
                .buck_error_context(format!(
                    "Reading value of a symlink to be copied {:?}",
                    src_path.as_ref()
                ))?
        } else {
            // Force "copied" symlink to be absolute as it points outside of original output we are copying.
            symlink_target_abs_path.into_path_buf()
        }
    };
    fs_util::symlink(&symlink_target, &dst_path)
        .categorize_tagged(ErrorTag::CopyOutputs)
        .buck_error_context(format!(
            "Creating symlink at {:?} pointing to {:?}",
            dst_path.as_ref(),
            symlink_target
        ))?;

    Ok(())
}

/// Copies a directory to the output path, rooted at `dst`.
async fn copy_directory<P, Q>(src: P, dst: Q, context: &CopyContext) -> buck2_error::Result<()>
where
    P: AsRef<AbsPath>,
    Q: AsRef<AbsPath>,
{
    let (sender, receiver) = mpsc::channel(COPY_DIRECTORY_CONCURRENCY);
    let stopped = Arc::new(AtomicBool::new(false));
    let producer_stopped = stopped.clone();
    let source = src.as_ref().to_owned();
    let destination = dst.as_ref().to_owned();

    let producer = async move {
        let result = enqueue_directory(source, destination, &sender, &producer_stopped).await;
        if let Err(error) = result
            && !producer_stopped.load(Ordering::Acquire)
        {
            let _ignored = sender.send(CopyQueueItem::Error(error)).await;
        }
    };
    let consumer = run_copy_work_queue(receiver, context, stopped);
    let ((), result) = tokio::join!(producer, consumer);
    result
}

#[async_recursion::async_recursion]
async fn enqueue_directory(
    source: AbsPathBuf,
    destination: AbsPathBuf,
    sender: &mpsc::Sender<CopyQueueItem>,
    stopped: &AtomicBool,
) -> buck2_error::Result<()> {
    if stopped.load(Ordering::Acquire) {
        return Ok(());
    }

    let (ready, wait_until_ready) = oneshot::channel();
    if sender
        .send(CopyQueueItem::Work(CopyWork::Directory {
            destination: destination.clone(),
            ready,
        }))
        .await
        .is_err()
        || wait_until_ready.await.is_err()
    {
        return Ok(());
    }

    let stream = tokio_stream::wrappers::ReadDirStream::new(
        tokio::fs::read_dir(&source)
            .await
            .buck_error_context(format!("reading directory {source:?}"))?,
    )
    .err_into::<buck2_error::Error>();
    futures::pin_mut!(stream);

    while let Some(entry) = stream.try_next().await? {
        if stopped.load(Ordering::Acquire) {
            return Ok(());
        }

        let entry_source = AbsPathBuf::new(entry.path())?;
        let entry_destination = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .await
            .buck_error_context(format!("Inspecting output {entry_source:?}"))?;
        if file_type.is_dir() {
            enqueue_directory(entry_source, entry_destination, sender, stopped)
                .await
                .buck_error_context(format!("Copying subdirectory {:?}", entry.path()))?;
        } else {
            let work = if file_type.is_symlink() {
                CopyWork::Symlink {
                    source: entry_source,
                    destination: entry_destination,
                }
            } else {
                CopyWork::File {
                    source: entry_source,
                    destination: entry_destination,
                }
            };
            if sender.send(CopyQueueItem::Work(work)).await.is_err() {
                return Ok(());
            }
        }
    }

    Ok(())
}

async fn run_copy_work_queue(
    receiver: mpsc::Receiver<CopyQueueItem>,
    context: &CopyContext,
    stopped: Arc<AtomicBool>,
) -> buck2_error::Result<()> {
    // Do not short-circuit this stream. A dropped `spawn_blocking` handle does not cancel its
    // task, so every admitted filesystem mutation must finish before this function returns.
    let first_error = tokio_stream::wrappers::ReceiverStream::new(receiver)
        .map(|item| run_copy_queue_item(item, context, &stopped))
        .buffer_unordered(COPY_DIRECTORY_CONCURRENCY)
        .fold(None, |first_error, result| async {
            match (first_error, result) {
                (None, Err(error)) => {
                    stopped.store(true, Ordering::Release);
                    Some(error)
                }
                (first_error, _) => first_error,
            }
        })
        .await;

    first_error.map_or(Ok(()), Err)
}

async fn run_copy_queue_item(
    item: CopyQueueItem,
    context: &CopyContext,
    stopped: &AtomicBool,
) -> buck2_error::Result<()> {
    if stopped.load(Ordering::Acquire) {
        return Ok(());
    }

    match item {
        CopyQueueItem::Work(work) => run_copy_work(work, context).await,
        CopyQueueItem::Error(error) => Err(error),
    }
}

async fn run_copy_work(work: CopyWork, context: &CopyContext) -> buck2_error::Result<()> {
    match work {
        CopyWork::Directory { destination, ready } => {
            tokio::fs::create_dir_all(&destination)
                .await
                .buck_error_context(format!("Creating output directory {destination:?}"))?;
            let _ignored = ready.send(());
            Ok(())
        }
        CopyWork::File {
            source,
            destination,
        } => {
            tokio::fs::copy(&source, &destination)
                .await
                .buck_error_context(format!("Copying file {source:?}"))?;
            make_writable(&destination)
        }
        CopyWork::Symlink {
            source,
            destination,
        } => {
            let source_for_error = source.clone();
            let copy_context = context.clone();
            tokio::task::spawn_blocking(move || copy_symlink(&source, &destination, &copy_context))
                .await
                .buck_error_context(format!("Copying symlink {source_for_error:?}"))??;
            Ok(())
        }
    }
}

async fn copy_file(src: &Path, dst: &Path) -> buck2_error::Result<()> {
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let dest_path = match dst.is_dir() {
        true => Cow::Owned(
            dst.join(
                src.file_name()
                    .internal_error("Failed getting output name")?,
            ),
        ),
        false => Cow::Borrowed(dst),
    };

    // NOTE: We don't do the overwrite since we might be writing to e.g. a pipe here and we can't
    // do an atomic move into it.
    match tokio::fs::copy(src, &dest_path).await {
        Ok(..) => make_writable(&dest_path),
        Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
            let dir = dest_path
                .parent()
                .internal_error("Output path has no parent")?;
            let mut tmp_name = dest_path
                .file_name()
                .internal_error("Output path has no file name")?
                .to_owned();
            tmp_name.push(".buck2.tmp");
            let tmp_path = dir.join(tmp_name);
            tokio::fs::copy(src, &tmp_path).await?;
            tokio::fs::rename(&tmp_path, &dest_path).await?;
            make_writable(&dest_path)
        }
        Err(e) => Err(convert_broken_pipe_error(e)),
    }
}

/// A copy carries the output's mode, which may be read-only (`buck2.readonly_outputs`), and a
/// file outside buck-out is the user's to write. Nothing is done to what is not a plain file, a
/// device `--out` was pointed at for one.
fn make_writable(path: &Path) -> buck2_error::Result<()> {
    let path = AbsPath::new(path)?;
    if fs_util::symlink_metadata(path)
        .categorize_internal()?
        .is_file()
    {
        fs_util::set_writable(path, true).categorize_internal()?;
    }
    Ok(())
}

fn convert_broken_pipe_error(e: io::Error) -> buck2_error::Error {
    buck2_error::Error::from(ClientIoError::from(e))
        .context("Error writing build artifact to --out")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    use buck2_fs::fs_util::uncategorized as fs_util;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_copies_of_read_only_outputs_are_writable() -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        let dst_dir = tempfile::tempdir()?;
        std::fs::create_dir_all(src_dir.path().join("dir"))?;
        for p in ["dir/sealed", "sealed"] {
            let path = src_dir.path().join(p);
            std::fs::write(&path, "content")?;
            fs_util::set_writable(AbsPath::new(&path)?, false)?;
        }
        let (src, dst) = (src_dir.path().join("dir"), dst_dir.path().join("dir"));
        let copy_context = CopyContext {
            relative_symlink_boundary: fs_util::canonicalize(AbsPath::new(src_dir.path())?)?,
        };
        copy_directory(AbsPath::new(&src)?, AbsPath::new(&dst)?, &copy_context).await?;
        let (src, dst) = (src_dir.path().join("sealed"), dst_dir.path().join("sealed"));
        copy_file(&src, &dst).await?;
        // Over the previous copy, as a second `--out` to the same path goes.
        copy_file(&src, &dst).await?;

        for p in ["dir/sealed", "sealed"] {
            let copied = std::fs::metadata(dst_dir.path().join(p))?;
            assert!(!copied.permissions().readonly(), "{p}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_directory() -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        // <tmp>
        // ├── bar
        // │    ├── qux
        // │    │    └── buzz
        // │    ├── foo_in_bar -> ../foo
        // │    └── some_file
        // ├── bax -> bar/qux
        // ├── foo
        // ├── foo_abs -> <tmp>/foo
        // └── fool -> foo
        std::fs::create_dir_all(src_dir.path().join("bar/qux"))?;
        std::fs::write(src_dir.path().join("foo"), "some content")?;
        std::fs::write(src_dir.path().join("bar/some_file"), "more content")?;
        std::fs::write(src_dir.path().join("bar/qux/buzz"), "even more")?;
        let fool_path = src_dir.path().join("fool");
        let foo_abs_path = src_dir.path().join("foo_abs");
        let foo_in_bar_path = src_dir.path().join("bar/foo_in_bar");
        let bax_path = src_dir.path().join("bax");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("foo", &fool_path)?;
            std::os::unix::fs::symlink(src_dir.path().join("foo"), &foo_abs_path)?;
            std::os::unix::fs::symlink("bar/qux", &bax_path)?;
            std::os::unix::fs::symlink("../foo", &foo_in_bar_path)?;
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file("foo", &fool_path)?;
            std::os::windows::fs::symlink_file(src_dir.path().join("foo"), &foo_abs_path)?;
            std::os::windows::fs::symlink_dir("bar\\qux", &bax_path)?;
            std::os::windows::fs::symlink_file("..\\foo", &foo_in_bar_path)?;
        }

        let dst_dir = tempfile::tempdir()?;
        let src_path = AbsPath::new(src_dir.path())?;
        let dst_path = AbsPath::new(dst_dir.path())?;
        let copy_context = CopyContext {
            relative_symlink_boundary: fs_util::canonicalize(src_path)?,
        };
        copy_directory(src_path, dst_path, &copy_context).await?;

        assert!(dst_dir.path().join("foo").is_file());
        assert!(dst_dir.path().join("bar/some_file").is_file());
        assert!(dst_dir.path().join("bar/qux/buzz").is_file());

        let copied_fool_path = std::fs::read_link(dst_dir.path().join("fool"))?;
        #[cfg(unix)]
        {
            assert_eq!(PathBuf::from("foo"), copied_fool_path);
        }
        #[cfg(windows)]
        {
            // Symlink value is canonicalized on Windows
            let dst_dir_canon_path = fs_util::canonicalize(dst_path)?;
            assert_eq!(dst_dir_canon_path.as_path().join("foo"), copied_fool_path);
        }

        let copied_foo_abs_path = std::fs::read_link(dst_dir.path().join("foo_abs"))?;
        #[cfg(unix)]
        {
            assert_eq!(src_dir.path().join("foo"), copied_foo_abs_path);
        }
        #[cfg(windows)]
        {
            // Symlink value is canonicalized on Windows
            let src_dir_canon_path = fs_util::canonicalize(src_path)?;
            assert_eq!(
                src_dir_canon_path.as_path().join("foo"),
                copied_foo_abs_path
            );
        }

        let copied_bax_path = std::fs::read_link(dst_dir.path().join("bax"))?;
        #[cfg(unix)]
        {
            assert_eq!(PathBuf::from("bar/qux"), copied_bax_path);
        }
        #[cfg(windows)]
        {
            // Symlink value is canonicalized on Windows
            let dst_dir_canon_path = fs_util::canonicalize(dst_path)?;
            assert_eq!(
                dst_dir_canon_path.as_path().join("bar\\qux"),
                copied_bax_path
            );
        }

        let copied_foo_in_bar_path = std::fs::read_link(dst_dir.path().join("bar/foo_in_bar"))?;
        #[cfg(unix)]
        {
            assert_eq!(PathBuf::from("../foo"), copied_foo_in_bar_path);
        }
        #[cfg(windows)]
        {
            // Symlink value is canonicalized on Windows
            let dst_dir_canon_path = fs_util::canonicalize(dst_path)?;
            assert_eq!(
                dst_dir_canon_path.as_path().join("foo"),
                copied_foo_in_bar_path
            );
        }

        // Second time to check everything overwrites fine
        copy_directory(src_path, dst_path, &copy_context)
            .await
            .buck_error_context("copy second time")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_directory_propagates_traversal_error() -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        let src_path = AbsPath::new(src_dir.path())?;
        let missing_src = src_path.join("missing");
        let dst_dir = tempfile::tempdir()?;
        let dst_path = AbsPath::new(dst_dir.path())?;
        let copy_context = CopyContext {
            relative_symlink_boundary: fs_util::canonicalize(src_path)?,
        };

        let error = copy_directory(&missing_src, dst_path, &copy_context)
            .await
            .expect_err("copying a missing source directory should fail");

        assert!(error.to_string().contains("reading directory"));
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_directory_propagates_copy_error() -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        std::fs::write(src_dir.path().join("foo"), "some content")?;
        let src_path = AbsPath::new(src_dir.path())?;

        let dst_dir = tempfile::tempdir()?;
        std::fs::create_dir(dst_dir.path().join("foo"))?;
        let dst_path = AbsPath::new(dst_dir.path())?;
        let copy_context = CopyContext {
            relative_symlink_boundary: fs_util::canonicalize(src_path)?,
        };

        let error = copy_directory(src_path, dst_path, &copy_context)
            .await
            .expect_err("copying a file over a directory should fail");

        assert!(error.to_string().contains("Copying file"));
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_directory_with_symlink_pointing_externally() -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        let src_path = fs_util::canonicalize(AbsPath::new(src_dir.path())?)?;
        // <tmp>
        // ├── qux
        // │    ├── foo -> ../foo
        // │    └── bar -> ../bar
        // ├── bar
        // │    └── baz
        // └── foo
        std::fs::write(src_path.as_path().join("foo"), "some content")?;
        std::fs::create_dir_all(src_path.as_path().join("bar"))?;
        std::fs::write(src_path.as_path().join("bar/baz"), "some content")?;
        std::fs::create_dir_all(src_path.as_path().join("qux"))?;
        let foo_link_path = src_path.as_path().join("qux/foo");
        let bar_link_path = src_path.as_path().join("qux/bar");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../foo", &foo_link_path)?;
            std::os::unix::fs::symlink("../bar", &bar_link_path)?;
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file("..\\foo", &foo_link_path)?;
            std::os::windows::fs::symlink_dir("..\\bar", &bar_link_path)?;
        }
        let dst_dir = tempfile::tempdir()?;
        let dst_path = AbsPath::new(dst_dir.path())?;
        let copy_context = CopyContext {
            relative_symlink_boundary: src_path.join(ForwardRelativePath::unchecked_new("qux")),
        };
        copy_directory(&src_path, dst_path, &copy_context).await?;

        // Check both symlinks are valid and are absolute.

        let copied_foo_target = std::fs::read_link(dst_dir.path().join("qux/foo"))?;
        assert_eq!(src_path.as_path().join("foo"), copied_foo_target);

        let copied_bar_target = std::fs::read_link(dst_dir.path().join("qux/bar"))?;
        assert_eq!(src_path.as_path().join("bar"), copied_bar_target);

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_symlink() -> buck2_error::Result<()> {
        test_copy_symlink_parametrized(&|_| Ok(()), false)?;
        test_copy_symlink_parametrized(&|_| Ok(()), true)?;
        // Check that symlink overwrites regular file in output directory
        let setup_file_to_overwrite = |dst_dir: &TempDir| {
            std::fs::write(dst_dir.path().join("foo_copy"), "some content")?;
            Ok(())
        };
        test_copy_symlink_parametrized(&setup_file_to_overwrite, false)?;
        test_copy_symlink_parametrized(&setup_file_to_overwrite, true)?;
        // Check that symlink overwrites directory in output directory
        let setup_directory_with_content_to_overwrite = |dst_dir: &TempDir| {
            std::fs::create_dir(dst_dir.path().join("foo_copy"))?;
            std::fs::write(dst_dir.path().join("foo_copy/some_file"), "some content")?;
            Ok(())
        };
        test_copy_symlink_parametrized(&setup_directory_with_content_to_overwrite, false)?;
        test_copy_symlink_parametrized(&setup_directory_with_content_to_overwrite, true)?;
        Ok(())
    }

    fn test_copy_symlink_parametrized(
        prepare_dst_dir: &dyn Fn(&TempDir) -> buck2_error::Result<()>,
        is_directory_symlink: bool,
    ) -> buck2_error::Result<()> {
        let src_dir = tempfile::tempdir()?;
        let src_symlink_target_path = src_dir.path().join("bar");
        let src_symlink_path = src_dir.path().join("foo");

        if is_directory_symlink {
            std::fs::create_dir(&src_symlink_target_path)?;
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir("bar", &src_symlink_path)?;
            }
        } else {
            std::fs::write(&src_symlink_target_path, "some content")?;
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file("bar", &src_symlink_path)?;
            }
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("bar", &src_symlink_path)?;
        }

        let dst_dir = tempfile::tempdir()?;
        prepare_dst_dir(&dst_dir)?;

        let dst_symlink_path = dst_dir.path().join("foo_copy");

        copy_symlink(
            AbsPath::new(&src_symlink_path)?,
            AbsPath::new(&dst_symlink_path)?,
            &CopyContext {
                relative_symlink_boundary: fs_util::canonicalize(AbsPath::new(src_dir.path())?)?,
            },
        )?;

        let target = std::fs::read_link(&dst_symlink_path)?;
        #[cfg(unix)]
        {
            assert_eq!(PathBuf::from("bar"), target);
        }
        #[cfg(windows)]
        {
            // Symlink value is canonicalized on Windows
            let dst_dir_canon_path = fs_util::canonicalize(AbsPath::new(dst_dir.path())?)?;
            assert_eq!(dst_dir_canon_path.as_path().join("bar"), target);
        }

        Ok(())
    }

    #[cfg(unix)]
    mod unix {

        use assert_matches::assert_matches;

        use super::super::*;

        #[tokio::test]
        async fn test_copy_file() -> buck2_error::Result<()> {
            let dir = tempfile::tempdir()?;
            let out = dir.path().join("sleep");

            let res = buck2_util::process::async_background_command("cp")
                .arg(Path::new("/bin/sleep"))
                .arg(&out)
                .spawn()?
                .wait()
                .await?;

            assert!(res.success());

            let mut proc = buck2_util::process::async_background_command(&out)
                .arg("10000")
                .kill_on_drop(true)
                .spawn()
                .buck_error_context("Error spawning")?;

            // This will fail if we don't handle ETXTBSY.
            copy_file(Path::new("/bin/sleep"), &out).await?;

            // Check that our sleep didn't end
            assert_matches!(proc.try_wait(), Ok(None));

            Ok(())
        }
    }
}
