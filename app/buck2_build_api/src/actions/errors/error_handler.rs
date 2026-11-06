/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::fmt::Display;

use allocative::Allocative;
use buck2_data::ActionSubError;
use buck2_data::CommandExecution;
use console::strip_ansi_codes;
use derive_more::Display;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_value::StarlarkArtifactValue;

pub(crate) type ActionSubErrorResult<'a> = ValueOf<'a, ListType<&'a StarlarkActionSubError>>;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub(crate) enum ActionErrorHandlerError {
    #[error("Error handler failed. Expected return type `{0}`, got value with type `{1}`")]
    TypeError(Ty, String),
}

#[derive(ProvidesStaticType, Trace, Allocative, Debug, Display, NoSerialize)]
#[display(
     "ActionErrorCtx(stderr: {}, stdout: {})",
     self.stderr,
     self.stdout
 )]
pub struct StarlarkActionErrorContext<'v> {
    stderr: String,
    stdout: String,
    output_artifacts: Option<OutputArtifacts<'v>>,
}

/// The outputs an error handler may read, by artifact.
pub type OutputArtifacts<'v> =
    ValueOf<'v, DictType<ValueTyped<'v, StarlarkArtifact>, ValueTyped<'v, StarlarkArtifactValue>>>;

impl<'v> StarlarkActionErrorContext<'v> {
    pub(crate) fn new_from_command_execution(
        command: Option<&CommandExecution>,
        output_artifacts: Option<OutputArtifacts<'v>>,
    ) -> Self {
        let stderr = command.map_or(String::default(), |c| {
            c.details.as_ref().map_or(String::default(), |c| {
                strip_ansi_codes(&c.cmd_stderr).to_string()
            })
        });
        let stdout = command.map_or(String::default(), |c| {
            c.details.as_ref().map_or(String::default(), |c| {
                strip_ansi_codes(&c.cmd_stdout).to_string()
            })
        });

        StarlarkActionErrorContext {
            stderr,
            stdout,
            output_artifacts,
        }
    }
}

impl<'v> AllocValue<'v> for StarlarkActionErrorContext<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

starlark::methods_static!(ACTION_ERROR_CONTEXT_METHODS = action_error_context_methods);

#[starlark_value(type = "ActionErrorCtx", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkActionErrorContext<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(ACTION_ERROR_CONTEXT_METHODS.methods())
    }
}

/// Context object passed to action error handlers containing information about the failed action.
///
/// `ActionErrorCtx` provides access to the stdout, stderr, and optionally output artifacts
/// from a failed action. Error handler functions receive this context and use it to analyze
/// the failure and produce structured [`ActionSubError`](../ActionSubError) objects for better
/// error diagnostics and categorization.
///
/// The context object has three main attributes:
/// - `stderr`: The stderr output from the failed action (as a string)
/// - `stdout`: The stdout output from the failed action (as a string)
/// - `output_artifacts`: A dictionary of output artifacts (if `outputs_for_error_handler` was specified in [`AnalysisActions.run`](../AnalysisActions/#analysisactionsrun))
///
/// Error handlers use this information to create categorized errors via `new_sub_error()` or
/// parse compiler output using `parse_with_errorformat()`.
///
/// Example:
/// ```python
/// def my_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
///     errors = []
///
///     # Check stderr for specific error patterns
///     if "compilation failed" in ctx.stderr.lower():
///         errors.append(ctx.new_sub_error(
///             category = "compilation_error",
///             message = "Compilation failed - check syntax and imports",
///             show_in_stderr = True,
///         ))
///
///     # Parse structured error output using errorformat
///     if "error:" in ctx.stderr:
///         # Parse errors like "file.rs:42:10: error: expected semicolon"
///         parsed_errors = ctx.parse_with_errorformat(
///             category = "syntax_error",
///             error = ctx.stderr,
///             errorformats = ["%f:%l:%c: %m"],
///         )
///         errors.extend(parsed_errors)
///
///     # Access output artifacts for structured error data
///     if ctx.output_artifacts:
///         for artifact, value in ctx.output_artifacts.items():
///             error_json = value.read_json()
///             if "errors" in error_json:
///                 for err in error_json["errors"]:
///                     errors.append(ctx.new_sub_error(
///                         category = err.get("code", "unknown"),
///                         message = err.get("message"),
///                         file = err.get("file"),
///                         lnum = err.get("line"),
///                     ))
///
///     return errors
///
/// # Use the error handler in an action
/// def _impl(ctx):
///     ctx.actions.run(
///         cmd_args(["compiler", "main.rs"]),
///         category = "compile",
///         error_handler = my_error_handler,
///     )
/// ```
#[starlark_module]
fn action_error_context_methods(builder: &mut MethodsBuilder) {
    /// Retrieve the stderr of the failed action.
    /// Can use string/regex matching to identify the error in order to categorize it.
    #[starlark(attribute)]
    fn stderr<'v>(this: &'v StarlarkActionErrorContext) -> starlark::Result<&'v str> {
        Ok(&this.stderr)
    }

    /// Retrieve the stdout of the failed action.
    /// Can use string/regex matching to identify the patterns in order to categorize it.
    #[starlark(attribute)]
    fn stdout<'v>(this: &'v StarlarkActionErrorContext) -> starlark::Result<&'v str> {
        Ok(&this.stdout)
    }

    /// Allows the output artifacts to be retrieve if [`outputs_for_error_handler`](../AnalysisActions/#analysisactionsrun)
    /// is set and the output artifact exists. This is useful for languages with structured error output, making the error retrieval process simpler.
    ///
    /// This is also the recommended way to retrieve file path and line number, as reliably extracting that information
    /// from stdout/stderr can be challenging
    #[starlark(attribute)]
    fn output_artifacts<'v>(
        this: &'v StarlarkActionErrorContext<'v>,
    ) -> starlark::Result<NoneOr<OutputArtifacts<'v>>> {
        Ok(NoneOr::from_option(this.output_artifacts))
    }

    /// Create a new sub error, specifying an error category name, optional message, and
    /// optional location information.
    ///
    /// The category should be finer grain error categorizations provided by the rule authors,
    /// and tend to be language specific. These should not be any kind of shared concepts
    /// among all errors for all languages/rules. For example, timeouts and infra errors
    /// should not go here - buck2 tries to categorize these types of errors automatically.
    /// An example of a finer grain error category may be the error code for rustc outputs.
    ///
    /// 'category': Required, useful for providing a more granular error category for action errors.
    /// 'message': Optional, provide users with additional context about the error to help with debugging/understandability/resolution, etc.
    /// 'file': Optional, file path where the error occurred.
    /// 'lnum': Optional, line number where the error occurred.
    /// 'end_lnum': Optional, end line number for multi-line error spans.
    /// 'col': Optional, column number where the error occurred.
    /// 'end_col': Optional, end column number for error ranges.
    /// 'error_type': Optional, type of error (e.g., error, warning, info).
    /// 'error_number': Optional, numeric error code.
    /// 'show_in_stderr': Optional, whether to show this error in stderr (default: false).
    ///
    /// The message will be emitted to the build report, and to the stderr in the error diagnostics
    /// section.
    fn new_sub_error<'v>(
        #[starlark(this)] _this: &'v StarlarkActionErrorContext,
        #[starlark(require = named)] category: String,
        #[starlark(require = named, default = NoneOr::None)] message: NoneOr<String>,
        #[starlark(require = named, default = NoneOr::None)] file: NoneOr<String>,
        #[starlark(require = named, default = NoneOr::None)] lnum: NoneOr<u64>,
        #[starlark(require = named, default = NoneOr::None)] end_lnum: NoneOr<u64>,
        #[starlark(require = named, default = NoneOr::None)] col: NoneOr<u64>,
        #[starlark(require = named, default = NoneOr::None)] end_col: NoneOr<u64>,
        #[starlark(require = named, default = NoneOr::None)] error_type: NoneOr<String>,
        #[starlark(require = named, default = NoneOr::None)] error_number: NoneOr<u64>,
        #[starlark(require = named, default = false)] show_in_stderr: bool,
        #[starlark(require = named, default = NoneOr::None)] subcategory: NoneOr<String>,
        #[starlark(require = named, default = NoneOr::None)] remediation: NoneOr<String>,
    ) -> starlark::Result<StarlarkActionSubError> {
        Ok(StarlarkActionSubError {
            category: RefCell::new(category),
            message: message.into_option(),
            file: file.into_option(),
            lnum: lnum.into_option(),
            end_lnum: end_lnum.into_option(),
            col: col.into_option(),
            end_col: end_col.into_option(),
            error_type: error_type.into_option(),
            error_number: error_number.into_option(),
            show_in_stderr: RefCell::new(show_in_stderr),
            subcategory: RefCell::new(subcategory.into_option()),
            remediation: RefCell::new(remediation.into_option()),
        })
    }

    /// Parse error text using vim errorformat patterns to create structured error information.
    /// This method leverages vim's proven errorformat system to extract file paths, line numbers,
    /// and error messages from compiler/tool output, automatically creating ActionSubError objects.
    ///
    /// For errorformat pattern syntax, see: https://neovim.io/doc/user/quickfix.html#errorformat
    ///
    /// Multiple patterns can be provided and will be tried in order until one matches.
    /// This is useful for tools that may output errors in different formats.
    ///
    /// Args:
    /// - `category`: Base category name for the generated sub-errors (e.g., "rust", "gcc")
    /// - `error`: The error text to parse (typically stderr or stdout from the failed action)
    /// - `errorformats`: List of vim errorformat pattern strings to try matching against
    ///
    /// Returns a list of ActionSubError objects with structured error information including
    /// file locations when successfully parsed from the error text.
    fn parse_with_errorformat<'v>(
        #[starlark(this)] _this: &'v StarlarkActionErrorContext,
        #[starlark(require = named)] category: String,
        #[starlark(require = named)] error: String,
        #[starlark(require = named)] errorformats: ValueOf<'v, ListOrTupleType<String>>,
        _heap: Heap<'v>,
    ) -> starlark::Result<Vec<StarlarkActionSubError>> {
        let error_lines = buck2_errorformat::split_lines(&error);
        let error_entries =
            buck2_errorformat::parse_error_format(errorformats.try_to_vec()?, error_lines)
                .map_err(buck2_error::Error::from)?;
        let res = error_entries
            .into_iter()
            .map(|e| StarlarkActionSubError::from_errorformat_entry(e, category.clone()))
            .collect();
        Ok(res)
    }
}

#[derive(ProvidesStaticType, Trace, Allocative, Debug, NoSerialize)]
pub(crate) struct StarlarkActionSubError {
    category: RefCell<String>,
    message: Option<String>,

    // file path for the error location
    file: Option<String>,
    // Line number
    lnum: Option<u64>,
    // End line (for multi-line spans)
    end_lnum: Option<u64>,
    //  Column number
    col: Option<u64>,
    // End column (for ranges)
    end_col: Option<u64>,
    // Type of error (error, warning, info, etc.)
    error_type: Option<String>,
    // Numeric error code (e.g., 404, 500)
    error_number: Option<u64>,
    // Whether to show this error in stderr
    show_in_stderr: RefCell<bool>,
    // Subcategory for finer-grained categorization
    subcategory: RefCell<Option<String>>,
    // Remediation steps for the error
    remediation: RefCell<Option<String>>,
}

impl Display for StarlarkActionSubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ActionSubError(category={}", self.category.borrow())?;

        if let Some(ref message) = self.message {
            write!(f, ", message={}", message)?;
        }

        if let Some(ref file) = self.file {
            write!(f, ", file={}", file)?;
        }

        if let Some(lnum) = self.lnum {
            write!(f, ", lnum={}", lnum)?;
        }

        if let Some(end_lnum) = self.end_lnum {
            write!(f, ", end_lnum={}", end_lnum)?;
        }

        if let Some(col) = self.col {
            write!(f, ", col={}", col)?;
        }

        if let Some(end_col) = self.end_col {
            write!(f, ", end_col={}", end_col)?;
        }

        if let Some(ref error_type) = self.error_type {
            write!(f, ", error_type={}", error_type)?;
        }

        if let Some(error_number) = self.error_number {
            write!(f, ", error_number={}", error_number)?;
        }

        write!(f, ", show_in_stderr={}", self.show_in_stderr.borrow())?;

        if let Some(ref subcategory) = *self.subcategory.borrow() {
            write!(f, ", subcategory={}", subcategory)?;
        }

        if let Some(ref remediation) = *self.remediation.borrow() {
            write!(f, ", remediation={}", remediation)?;
        }

        write!(f, ")")
    }
}

impl PartialEq for StarlarkActionSubError {
    fn eq(&self, other: &Self) -> bool {
        *self.category.borrow() == *other.category.borrow()
            && self.message == other.message
            && self.file == other.file
            && self.lnum == other.lnum
            && self.end_lnum == other.end_lnum
            && self.col == other.col
            && self.end_col == other.end_col
            && self.error_type == other.error_type
            && self.error_number == other.error_number
            && *self.show_in_stderr.borrow() == *other.show_in_stderr.borrow()
            && *self.subcategory.borrow() == *other.subcategory.borrow()
            && *self.remediation.borrow() == *other.remediation.borrow()
    }
}

impl Eq for StarlarkActionSubError {}

impl StarlarkActionSubError {
    pub(crate) fn from_errorformat_entry(
        entry: buck2_errorformat::Entry,
        category: String,
    ) -> Self {
        StarlarkActionSubError {
            category: RefCell::new(category),
            message: entry.message,
            file: entry.filename,
            lnum: entry.lnum.map(|x| x as u64),
            end_lnum: entry.end_lnum.map(|x| x as u64),
            col: entry.col.map(|x| x as u64),
            end_col: entry.end_col.map(|x| x as u64),
            error_type: entry.error_type,
            error_number: entry.error_number.map(|x| x as u64),
            show_in_stderr: RefCell::new(false),
            subcategory: RefCell::new(None),
            remediation: RefCell::new(None),
        }
    }
}

impl<'v> AllocValue<'v> for StarlarkActionSubError {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

starlark::methods_static!(ACTION_SUB_ERROR_METHODS = action_sub_error_methods);

#[starlark_value(
    type = "ActionSubError",
    StarlarkTypeRepr,
    UnpackValue,
    ty_vtable_no_freeze
)]
impl<'v> StarlarkValue<'v> for StarlarkActionSubError {
    fn get_methods() -> Option<&'static Methods> {
        Some(ACTION_SUB_ERROR_METHODS.methods())
    }

    fn set_attr(&self, attribute: &str, new_value: Value<'v>) -> starlark::Result<()> {
        match attribute {
            "category" => {
                let new_category = new_value.unpack_str_err()?;
                *self.category.borrow_mut() = new_category.to_owned();
                Ok(())
            }
            "show_in_stderr" => {
                let new_show_in_stderr = new_value
                    .unpack_bool()
                    .expect("show_in_stderr expected bool");
                *self.show_in_stderr.borrow_mut() = new_show_in_stderr;
                Ok(())
            }
            "subcategory" => {
                let new_subcategory = new_value.unpack_str_err()?;
                *self.subcategory.borrow_mut() = Some(new_subcategory.to_owned());
                Ok(())
            }
            "remediation" => {
                let new_remediation = new_value.unpack_str_err()?;
                *self.remediation.borrow_mut() = Some(new_remediation.to_owned());
                Ok(())
            }
            _ => ValueError::unsupported(self, &format!(".{attribute}=")),
        }
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        if let Some(other) = other.downcast_ref::<Self>() {
            Ok(self.eq(other))
        } else {
            Ok(false)
        }
    }
}

/// Represents a structured action sub-error.
///
/// `ActionSubError` captures detailed information about errors that occur during build actions,
/// including location information (file, line, column), error messages, and categorization.
/// These objects are created in error handlers (`error_handler` in [`AnalysisActions.run`](../AnalysisActions/#analysisactionsrun)) and are used to provide better error diagnostics.
///
/// ActionSubError objects are typically created using [`ActionErrorCtx.new_sub_error`](../ActionErrorCtx/#actionerrorctxnew_sub_error)
/// or parsed from error output using [`ActionErrorCtx.parse_with_errorformat`](../ActionErrorCtx/#actionerrorctxparse_with_errorformat).
#[starlark_module]
fn action_sub_error_methods(builder: &mut MethodsBuilder) {
    /// A more granular category for the action error.
    #[starlark(attribute)]
    fn category<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<String> {
        Ok(this.category.borrow().clone())
    }

    /// An optional message to be displayed with the error, used to provide additional context
    #[starlark(attribute)]
    fn message<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<&'v str>> {
        Ok(match &this.message {
            Some(message) => NoneOr::Other(message.as_str()),
            None => NoneOr::None,
        })
    }

    /// File path for the error location.
    #[starlark(attribute)]
    fn file<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<&'v str>> {
        Ok(match &this.file {
            Some(file) => NoneOr::Other(file.as_str()),
            None => NoneOr::None,
        })
    }

    /// Line number for the error location.
    #[starlark(attribute)]
    fn lnum<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<u64>> {
        Ok(NoneOr::from_option(this.lnum))
    }

    /// End line number for multi-line error spans.
    #[starlark(attribute)]
    fn end_lnum<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<u64>> {
        Ok(NoneOr::from_option(this.end_lnum))
    }

    /// Column number for the error location.
    #[starlark(attribute)]
    fn col<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<u64>> {
        Ok(NoneOr::from_option(this.col))
    }

    /// End column number for error ranges.
    #[starlark(attribute)]
    fn end_col<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<u64>> {
        Ok(NoneOr::from_option(this.end_col))
    }

    /// Type of error (e.g., error, warning, info).
    #[starlark(attribute)]
    fn error_type<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<&'v str>> {
        Ok(match &this.error_type {
            Some(error_type) => NoneOr::Other(error_type.as_str()),
            None => NoneOr::None,
        })
    }

    /// Numeric error code (e.g., 404, 500).
    #[starlark(attribute)]
    fn error_number<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<u64>> {
        Ok(NoneOr::from_option(this.error_number))
    }

    /// Whether to show this error in stderr.
    #[starlark(attribute)]
    fn show_in_stderr<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<bool> {
        Ok(*this.show_in_stderr.borrow())
    }

    /// Optional subcategory for finer-grained error categorization
    #[starlark(attribute)]
    fn subcategory<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<String>> {
        Ok(NoneOr::from_option(this.subcategory.borrow().clone()))
    }

    /// Optional remediation steps for the error
    #[starlark(attribute)]
    fn remediation<'v>(this: &'v StarlarkActionSubError) -> starlark::Result<NoneOr<String>> {
        Ok(NoneOr::from_option(this.remediation.borrow().clone()))
    }
}

impl StarlarkActionSubError {
    pub(crate) fn to_proto(&self) -> ActionSubError {
        ActionSubError {
            category: self.category.borrow().clone(),
            message: self.message.clone(),
            file: self.file.clone(),
            lnum: self.lnum,
            end_lnum: self.end_lnum,
            col: self.col,
            end_col: self.end_col,
            error_type: self.error_type.clone(),
            error_number: self.error_number,
            show_in_stderr: *self.show_in_stderr.borrow(),
            subcategory: self.subcategory.borrow().clone(),
            remediation: self.remediation.borrow().clone(),
        }
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkActionSubError as ActionSubError,
    StarlarkActionErrorContext<'_> as ActionErrorCtx
)]
pub(crate) fn register_action_error_types(globals: &mut GlobalsBuilder) {}

/// Global methods for testing starlark action error handler.
#[starlark_module]
pub(crate) fn register_action_error_handler_for_testing(builder: &mut GlobalsBuilder) {
    /// Global function to create a new `ActionErrorContext` for testing a starlark action error
    /// handler via `bxl_test`.
    fn new_test_action_error_ctx<'v>(
        #[starlark(require=named, default = "")] stderr: &str,
        #[starlark(require=named, default = "")] stdout: &str,
    ) -> starlark::Result<StarlarkActionErrorContext<'v>> {
        Ok(StarlarkActionErrorContext {
            stderr: stderr.to_owned(),
            stdout: stdout.to_owned(),
            output_artifacts: None,
        })
    }
}
