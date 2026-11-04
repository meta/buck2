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
use std::cell::RefCell;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use buck2_hash::BuckDashMap;
use buck2_hash::BuckFixedSortedSet;

use crate::env::buck2_env;

type StructuredErrorHandler = Box<
    dyn for<'a> Fn(
            &'a str,
            &buck2_error::Error,
            (&'a str, u32, u32),
            &Arc<SoftErrorContext>,
            StructuredErrorOptions,
        ) + Send
        + Sync
        + 'static,
>;

static HANDLER: OnceLock<StructuredErrorHandler> = OnceLock::new();

/// Supplies the soft-error context associated with the current command, when one exists.
pub type SoftErrorContextProvider =
    Box<dyn Fn() -> Option<Arc<SoftErrorContext>> + Send + Sync + 'static>;

static CONTEXT_PROVIDER: OnceLock<SoftErrorContextProvider> = OnceLock::new();
// Work with no command dispatcher shares one process-lifetime policy and emission quota.
static FALLBACK_CONTEXT: OnceLock<Arc<SoftErrorContext>> = OnceLock::new();

thread_local! {
    static THREAD_CONTEXT: RefCell<Option<Arc<SoftErrorContext>>> = const { RefCell::new(None) };
}

struct RestoreSoftErrorContext(Option<Arc<SoftErrorContext>>);

impl Drop for RestoreSoftErrorContext {
    fn drop(&mut self) {
        THREAD_CONTEXT.with(|context| {
            context.replace(self.0.take());
        });
    }
}

pub fn buck2_hard_error_env() -> buck2_error::Result<Option<&'static str>> {
    buck2_env!("BUCK2_HARD_ERROR")
}

#[derive(Debug, PartialEq, Eq)]
enum ShowSoftErrorConfig {
    Disabled,
    All,
    Selected(BuckFixedSortedSet<String>),
}

impl ShowSoftErrorConfig {
    fn should_show(&self, category: &str) -> bool {
        match self {
            Self::Disabled => false,
            Self::All => true,
            Self::Selected(s) => s.contains(category),
        }
    }
}

impl ShowSoftErrorConfig {
    fn parse(s: &str) -> Self {
        if s.is_empty() {
            return Self::Disabled;
        }
        let val = s.to_lowercase();
        if val == "true" || val == "1" {
            return Self::All;
        }
        if let Some(categories) = val.strip_prefix("only=") {
            return Self::Selected(categories.split(',').map(|c| c.trim().to_owned()).collect());
        }
        Self::Disabled
    }
}

pub fn buck2_show_soft_errors_env() -> buck2_error::Result<Option<&'static str>> {
    buck2_env!("BUCK2_SHOW_SOFT_ERRORS")
}

static HARD_ERROR_PANIC_ALLOWLIST: LazyLock<BuckFixedSortedSet<String>> = LazyLock::new(|| {
    BuckFixedSortedSet::from_iter(["spawn_version_control_collector_failed".to_owned()])
});

/// Records a non-fatal error to telemetry without failing the command.
///
/// Reports are counted per category in Logview ([internal
/// link](https://www.internalfb.com/logview/overview/buck2)), where the buck2 oncall watches them,
/// so a category should name one specific thing going wrong.
///
/// Under `BUCK2_HARD_ERROR`, which buck2's own tests set, the report fails the command instead: the
/// macro returns the error from the enclosing function, the way `?` would. A call site with nowhere
/// to return an error to uses [`soft_error_or_panic!`]. A report on an error path that is being
/// returned anyway is a [`tag_error!`] or [`tag_result!`].
///
/// [`deprecation!`] is a small specialization of this macro for deprecated codepaths.
///
/// ## Example
///
/// See [`StructuredErrorOptions`] for the options:
///
/// ```ignore
/// soft_error!("cas_missing", err);
/// soft_error!("dep_only_incompatible_custom", err, task: false);
/// ```
pub macro soft_error($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    {
        let _reported = $crate::soft_error::report!($category, $err $(, $k: $v)*)?;
    }
}

/// [`soft_error!`] for a call site with nowhere to return an error to: a `Drop` impl, a thread,
/// an event sink, a closure that hands back a plain value. Under `BUCK2_HARD_ERROR` the report
/// panics there.
///
/// ```ignore
/// soft_error_or_panic!("command_lease_release_failed", err);
/// ```
pub macro soft_error_or_panic($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    $crate::soft_error::panic_on_hard_error($crate::soft_error::report!(
        $category,
        $err
        $(, $k: $v)*
    ))
}

/// Log a use of a deprecated codepath.
///
/// A variant of `soft_error!` specifically for deprecations, generally of external-facing features.
///
/// The only distinction from `soft_error!` is that these get a separate logview prefix and require
/// passing an `owner` and `started` date.
///
/// The `owner` and `started` date are not directly used for anything; their purpose is to provide
/// gentle social encouragement to complete deprecations promptly. While it is not actively
/// monitored, as a general policy deprecations that have made no progress in six months are subject
/// to deletion without warning; they can be brought back when someone intends to work on them. If
/// there is a codepath that you think should return an error but you do not intend to take any
/// steps towards making that happen in the near future, a `FIXME` comment is typically the
/// appropriate step.
///
/// ## Example
///
/// ```ignore
/// deprecation!("platform_flavor", err, owner: "yournamehere", started: "2023-03-09");
/// ```
pub macro deprecation($category:expr, $err:expr, owner: $owner:expr, started: $started:expr $(, $k:ident : $v:expr)* $(,)?) {
    {
        let _reported = $crate::soft_error::report!(
            $category,
            $err,
            kind: $crate::soft_error::SoftErrorKind::Deprecation {
                owner: Some(::std::borrow::Cow::Borrowed($owner)),
                started: Some(::std::borrow::Cow::Borrowed($started)),
            },
            user_facing: false
            $(, $k: $v)*
        )?;
    }
}

/// [`deprecation!`] for a call site with nowhere to return an error to, as
/// [`soft_error_or_panic!`].
pub macro deprecation_or_panic($category:expr, $err:expr, owner: $owner:expr, started: $started:expr $(, $k:ident : $v:expr)* $(,)?) {
    $crate::soft_error::panic_on_hard_error($crate::soft_error::report!(
        $category,
        $err,
        kind: $crate::soft_error::SoftErrorKind::Deprecation {
            owner: Some(::std::borrow::Cow::Borrowed($owner)),
            started: Some(::std::borrow::Cow::Borrowed($started)),
        },
        user_facing: false
        $(, $k: $v)*
    ))
}

/// Emit a warning visible on console to the user that has invoked the command.
///
/// As a general principle, buck2 does not do warnings.
///
/// If you are going to use this, the bar to clear is extremely high: You must be reasonably
/// confident that the human or agent who sees this warning on console finds it immediately valuable
/// in a large fraction of each and every specific circumstance in which this warning is shown.
///
/// Warnings should never be about things the user didn't or won't change (such as code checked into
/// the repo) or that are out of their control (such as CLI flags when buck is invoked by a script).
///
/// Instead of a warning, consider:
///  - Returning an error with an appropriate message asking the user to clarify their intent, fix
///    their configuration, remove the use of a deprecated API, whatever.
///  - Using `soft_error!` to log evidence of unreliability or other exceptional circumstances that
///    do not necessarily adversely effect the user at this particular moment for later inspection
///    by the oncall or others.
///  - Making use of superconsole to report progress and the state of the invocation.
///
/// Warnings are additionally treated like soft errors in that they are logged for monitoring,
/// produce failures in tests, etc.
///
/// ## Example
///
/// ```ignore
/// warn_user!("eden_buck_out", err);
/// ```
pub macro warn_user($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    {
        let _reported =
            $crate::soft_error::report!($category, $err, user_facing: true $(, $k: $v)*)?;
    }
}

/// [`warn_user!`] for a call site with nowhere to return an error to, as
/// [`soft_error_or_panic!`].
pub macro warn_user_or_panic($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    $crate::soft_error::panic_on_hard_error($crate::soft_error::report!(
        $category,
        $err,
        user_facing: true
        $(, $k: $v)*
    ))
}

/// Reports the error and hands it back, for an error path that returns it anyway.
pub macro tag_error($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    match $crate::soft_error::report!($category, $err $(, $k: $v)*) {
        Ok(err) => err,
        Err(err) => err,
    }
}

/// [`tag_error!`] for the error of a `Result`, which is then returned unchanged.
pub macro tag_result($category:expr, $res:expr $(, $k:ident : $v:expr)* $(,)?) {
    $res.map_err(|err| $crate::soft_error::tag_error!($category, err $(, $k: $v)*))
}

/// The report behind the macros above, which exist to record the call site. A reporter with a
/// location of its own calls [`handle_soft_error`].
#[doc(hidden)]
pub macro report($category:expr, $err:expr $(, $k:ident : $v:expr)* $(,)?) {
    $crate::soft_error::handle_soft_error(
        $category,
        $err,
        (file!(), line!(), column!()),
        $crate::soft_error::StructuredErrorOptions {
            $($k: $v,)*
            ..Default::default()
        },
    )
}

/// What an `_or_panic` call site does with the hard-error policy: the error has nowhere to go,
/// so it is a panic.
pub fn panic_on_hard_error(reported: Result<buck2_error::Error, buck2_error::Error>) {
    if let Err(e) = reported {
        panic!("{e:#}");
    }
}

/// Who a report is for.
#[derive(Debug, Clone)]
pub enum SoftErrorKind {
    /// Something went wrong. The buck2 oncall watches these; the user sees the ones that are not
    /// quiet.
    SoftError,
    /// Something that is being removed was used. `owner` is the person driving the removal and
    /// `started` the day (`YYYY-MM-DD`) the deprecation went in; both are absent only for
    /// deprecations declared outside the code, where nothing records them.
    Deprecation {
        owner: Option<Cow<'static, str>>,
        started: Option<Cow<'static, str>>,
    },
}

pub struct StructuredErrorOptions {
    pub kind: SoftErrorKind,
    /// Also print the error to the console. Set by [`warn_user!`], which says who that is for.
    pub user_facing: bool,
    /// Create a task for this error.
    pub task: bool,
    pub daemon_in_memory_state_is_corrupted: bool,
    pub daemon_materializer_state_is_corrupted: bool,
    pub action_cache_is_corrupted: bool,
    // By default, we only get a handful of traces per error category in Logview.
    // This key, if specified, enables logging one trace per unique key using
    // the "trace cut" feature of Logview. Note that the dimensionality of this
    // key must not be too large otherwise it can bring significant capacity cost
    // and may even bring down Logview.
    pub low_cardinality_key_for_additional_logview_samples: Option<Box<dyn ToString>>,
}

impl Default for StructuredErrorOptions {
    fn default() -> Self {
        Self {
            kind: SoftErrorKind::SoftError,
            user_facing: false,
            task: true,
            daemon_in_memory_state_is_corrupted: false,
            daemon_materializer_state_is_corrupted: false,
            action_cache_is_corrupted: false,
            low_cardinality_key_for_additional_logview_samples: None,
        }
    }
}

/// What [`soft_error!`] and [`deprecation!`] expand to. Only for reporters whose kind or owner is
/// known at run time, such as deprecations declared in buckconfig or Starlark.
pub fn handle_soft_error(
    category: &str,
    err: buck2_error::Error,
    loc: (&'static str, u32, u32),
    options: StructuredErrorOptions,
) -> Result<buck2_error::Error, buck2_error::Error> {
    validate_logview_category(category)?;
    if let SoftErrorKind::Deprecation {
        started: Some(started),
        ..
    } = &options.kind
    {
        validate_deprecation_start(started)?;
    }

    let context = soft_error_context()?;

    let mut options = options;
    if context.show_soft_error_config.should_show(category) {
        options.user_facing = true;
    }

    // We want to limit each error to appearing at most 10 times in a build (no point spamming people)
    if context.should_emit(loc) {
        if let Some(handler) = HANDLER.get() {
            handler(category, &err, loc, &context, options);
        }
    }

    if context.hard_error_config.should_panic(category) {
        panic!(
            "Upgraded warning to panic via $BUCK2_HARD_ERROR\n {category}: {:?}",
            err
        );
    }
    if context.hard_error_config.should_hard_error(category) {
        return Err(err.context("Upgraded warning to failure via $BUCK2_HARD_ERROR"));
    }

    Ok(err)
}

fn soft_error_context() -> buck2_error::Result<Arc<SoftErrorContext>> {
    if let Some(context) = capture_soft_error_context() {
        return Ok(context);
    }

    if let Some(context) = FALLBACK_CONTEXT.get() {
        return Ok(context.clone());
    }

    let context = Arc::new(SoftErrorContext::from_environment()?);
    let _ignored = FALLBACK_CONTEXT.set(context.clone());
    Ok(FALLBACK_CONTEXT.get().cloned().unwrap_or(context))
}

/// Captures command-scoped policy without manufacturing a process fallback.
pub fn capture_soft_error_context() -> Option<Arc<SoftErrorContext>> {
    THREAD_CONTEXT
        .with(|context| context.borrow().clone())
        .or_else(|| CONTEXT_PROVIDER.get().and_then(|provider| provider()))
}

/// Installs captured command policy while executing synchronous work on another thread.
pub fn with_soft_error_context<R>(
    context: Option<Arc<SoftErrorContext>>,
    func: impl FnOnce() -> R,
) -> R {
    let Some(context) = context else {
        return func();
    };

    THREAD_CONTEXT.with(|current| {
        let previous = current.replace(Some(context));
        let _restore = RestoreSoftErrorContext(previous);
        func()
    })
}

pub fn initialize(
    handler: StructuredErrorHandler,
    context_provider: SoftErrorContextProvider,
) -> buck2_error::Result<()> {
    soft_error_context()?;

    if let Err(_e) = CONTEXT_PROVIDER.set(context_provider) {
        panic!("Cannot initialize SoftErrorContextProvider more than once");
    }

    if let Err(_e) = HANDLER.set(handler) {
        panic!("Cannot initialize StructuredErrorHandler handler more than once");
    }

    Ok(())
}

/// Policy and rate-limit state shared by work attributed to one command.
#[derive(Debug)]
pub struct SoftErrorContext {
    hard_error_config: HardErrorConfig,
    show_soft_error_config: ShowSoftErrorConfig,
    counts: BuckDashMap<(&'static str, u32, u32), AtomicUsize>,
    command_scoped: bool,
}

impl SoftErrorContext {
    /// Parses the client-provided soft-error policy for a command.
    pub fn new(hard_error_config: &str, show_soft_error_config: &str) -> buck2_error::Result<Self> {
        Ok(Self {
            hard_error_config: HardErrorConfig::from_str(hard_error_config)?,
            show_soft_error_config: ShowSoftErrorConfig::parse(show_soft_error_config),
            counts: BuckDashMap::default(),
            command_scoped: true,
        })
    }

    fn from_environment() -> buck2_error::Result<Self> {
        let mut context = Self::new(
            buck2_hard_error_env()?.unwrap_or_default(),
            buck2_show_soft_errors_env()?.unwrap_or_default(),
        )?;
        context.command_scoped = false;
        Ok(context)
    }

    /// Whether this context belongs to a client command rather than process-wide fallback work.
    pub fn is_command_scoped(&self) -> bool {
        self.command_scoped
    }

    fn should_emit(&self, loc: (&'static str, u32, u32)) -> bool {
        if let Some(count) = self.counts.get(&loc) {
            return increment_soft_error_counter(&count);
        }

        let count = self.counts.entry(loc).or_default();
        increment_soft_error_counter(&count)
    }
}

fn increment_soft_error_counter(count: &AtomicUsize) -> bool {
    count
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            (count < 10).then_some(count + 1)
        })
        .is_ok()
}

/// Parse either a boolean or `only=category1,category2`
#[derive(Debug, PartialEq, Eq)]
enum HardErrorConfig {
    Bool(bool),
    Selected(BuckFixedSortedSet<String>),
    Panic,
}

impl HardErrorConfig {
    fn should_panic(&self, category: &str) -> bool {
        match self {
            Self::Panic => !HARD_ERROR_PANIC_ALLOWLIST.contains(category),
            _ => false,
        }
    }

    fn should_hard_error(&self, category: &str) -> bool {
        match self {
            Self::Bool(v) => *v,
            Self::Selected(s) => s.contains(category),
            Self::Panic => true, // category is in HARD_ERROR_PANIC_ALLOWLIST, make it a normal hard error
        }
    }
}

impl FromStr for HardErrorConfig {
    type Err = InvalidHardErrorConfig;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::Bool(false));
        }

        if s == "panic" {
            return Ok(Self::Panic);
        }

        if let Ok(v) = s.parse() {
            return Ok(Self::Bool(v));
        }

        let mut parts = s.split('=');

        if let (Some("only"), Some(v), None) = (parts.next(), parts.next(), parts.next()) {
            return Ok(Self::Selected(
                v.split(',').map(|s| s.trim().to_owned()).collect(),
            ));
        }

        Err(InvalidHardErrorConfig(s.to_owned()))
    }
}

#[derive(buck2_error::Error, Debug)]
#[error(
    "Invalid hard error config: `{0}`\n\
    Valid examples: empty, `true`, `false`, `only=category1,category2`"
)]
#[buck2(tag = Input)]
struct InvalidHardErrorConfig(String);

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum InvalidSoftError {
    #[error("Invalid category, must be lower_snake_case, got `{0}`")]
    InvalidCategory(String),
    #[error("Invalid deprecation start, must be a `YYYY-MM-DD` date, got `{0}`")]
    InvalidDeprecationStart(String),
}

/// The start of a deprecation is a calendar day as `YYYY-MM-DD`; it is there to be read, so the
/// shape is all that is checked.
fn validate_deprecation_start(started: &str) -> buck2_error::Result<()> {
    let bytes = started.as_bytes();
    let well_formed = bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if well_formed {
        Ok(())
    } else {
        Err(InvalidSoftError::InvalidDeprecationStart(started.to_owned()).into())
    }
}

/// A category must be a-z with no consecutive underscores. Or we raise an error.
pub fn validate_logview_category(category: &str) -> buck2_error::Result<()> {
    let mut allow_underscore = false;
    for &x in category.as_bytes() {
        if x.is_ascii_lowercase() {
            allow_underscore = true;
        } else if allow_underscore && x == b'_' {
            allow_underscore = false;
        } else {
            // Go to the shared error path
            allow_underscore = false;
            break;
        }
    }
    if !allow_underscore {
        Err(InvalidSoftError::InvalidCategory(category.to_owned()).into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {

    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn test_hard_error() -> buck2_error::Result<()> {
        assert!(HardErrorConfig::from_str("true")?.should_hard_error("foo"));
        assert!(!HardErrorConfig::from_str("false")?.should_hard_error("foo"));
        assert_eq!(
            HardErrorConfig::Bool(false),
            HardErrorConfig::from_str("")?,
            "Empty string must parse to no hard errors"
        );
        assert!(!HardErrorConfig::from_str("")?.should_hard_error("foo"));

        assert!(HardErrorConfig::from_str("only=foo,bar")?.should_hard_error("foo"));
        assert!(!HardErrorConfig::from_str("only=foo,bar")?.should_hard_error("baz"));

        Ok(())
    }

    #[test]
    fn test_soft_error_contexts_are_isolated() -> buck2_error::Result<()> {
        let first = SoftErrorContext::new("true", "")?;
        let second = SoftErrorContext::new("false", "only=test_category")?;
        let loc = ("test.rs", 1, 1);

        assert!(first.hard_error_config.should_hard_error("test_category"));
        assert!(!first.show_soft_error_config.should_show("test_category"));
        assert!(!second.hard_error_config.should_hard_error("test_category"));
        assert!(second.show_soft_error_config.should_show("test_category"));

        for _ in 0..10 {
            assert!(first.should_emit(loc));
        }
        assert!(!first.should_emit(loc));
        assert!(second.should_emit(loc));

        Ok(())
    }

    #[test]
    fn test_soft_error_context_limits_concurrent_emission() -> buck2_error::Result<()> {
        let context = Arc::new(SoftErrorContext::new("", "")?);
        let emitted = std::thread::scope(|scope| {
            (0..4)
                .map(|_| {
                    let context = context.clone();
                    scope.spawn(move || {
                        (0..100)
                            .filter(|_| context.should_emit(("test.rs", 1, 1)))
                            .count()
                    })
                })
                .map(|handle| handle.join().expect("worker should not panic"))
                .sum::<usize>()
        });

        assert_eq!(10, emitted);
        Ok(())
    }

    #[test]
    fn test_soft_error_context_sync_scope_restores_nested_context() -> buck2_error::Result<()> {
        let outer = Arc::new(SoftErrorContext::new("", "")?);
        let inner = Arc::new(SoftErrorContext::new("", "")?);

        with_soft_error_context(Some(outer.clone()), || {
            with_soft_error_context(Some(inner.clone()), || {
                let observed = capture_soft_error_context()
                    .expect("inner context should be visible on the worker thread");
                assert!(Arc::ptr_eq(&inner, &observed));
            });
            let observed = capture_soft_error_context()
                .expect("outer context should be restored on the worker thread");
            assert!(Arc::ptr_eq(&outer, &observed));
        });

        assert!(capture_soft_error_context().is_none());
        Ok(())
    }

    #[test]
    fn test_validate_deprecation_start() {
        assert_matches!(validate_deprecation_start("2026-10-25"), Ok(_));
        assert_matches!(validate_deprecation_start("2026-1-25"), Err(_));
        assert_matches!(validate_deprecation_start("yesterday"), Err(_));
        assert_matches!(validate_deprecation_start("2026/10/25"), Err(_));
    }

    #[test]
    fn test_validate_logview_category() {
        assert_matches!(validate_logview_category("valid"), Ok(_));
        assert_matches!(validate_logview_category("a_valid_category"), Ok(_));
        assert_matches!(validate_logview_category(""), Err(_));
        assert_matches!(validate_logview_category("Invalid_because_capital"), Err(_));
        assert_matches!(validate_logview_category("some_1number"), Err(_));
        assert_matches!(validate_logview_category("two__underscore"), Err(_));
        assert_matches!(validate_logview_category("a-dash"), Err(_));
        assert_matches!(validate_logview_category("_leading_underscore"), Err(_));
        assert_matches!(validate_logview_category("trailing_underscore_"), Err(_));
    }
}
