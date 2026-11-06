---
id: build_report
title: Build Report
---

The build report is a JSON file that you can ask buck to output which contains
structured information about the result of your build. It is particularly
valuable for its reporting of _unsuccessful_ outcomes in addition to
_successful_ ones; usually, most use cases that only need to care about
successful outcomes are well served by direct usage of the CLI.

To request a build report, pass `--build-report <path>` to `buck build` on the
CLI.

You can also pass `--build-report-options` with a comma-separated list of
options to customize the build report:

- `fill-out-failures`: Fill out the `failures` field in the build report (for
  Buck1 backwards compatibility).
- `package-project-relative-paths`: Include the project-relative path of
  packages for built targets.
- `include-artifact-hash-information`: Include artifact hash information in the
  output.
- `exclude-action-error-diagnostics`: Exclude the `error_diagnostics` field from
  action errors in the build report. This can reduce the size of build reports
  when detailed error diagnostic information from action error handlers is not
  needed.
- `truncate-error-content`: Truncate error message content in the build report
  to reduce size. This applies the same truncation limits used for error logging
  (20KB per error message).

At a high level, the build report outputs information for each of the targets
that you requested to have built on the CLI. As a result, it may report
information for more than one configuration or subtarget of a target. For
example, this can happen if you passed `--target-platforms` or built `:target`
and `:target[sub]`.

## Schema

```python
BuildReport {
    # A unique ID identifying this buck invocation. Currently a UUID, however
    # that may change in the future.
    trace_id: str,

    # True if all requested targets built successfully.
    #
    # In a streaming build report each line carries one target, so this only
    # covers that target and any errors not associated with a target, and it
    # only means that no error has been recorded for them yet.
    success: bool,

    # The error category for the build, in case it failed. One of "USER", "INFRA", "ENVIRONMENT".
    error_category: str,

    # The absolute path to the project root
    project_root: Path,

    # The results of the build, categorized by unconfigured target along with
    # any potential modifiers from the `?modifier` syntax
    results: dict[TargetLabelWithModifiers, BuildReportEntry],

    # A cache for error message lookup. This is meant for deduplicating strings
    # that might otherwise appear many times in the build report and cause an
    # unnecessary size increase. They keys are used in other fields in the build
    # report in reference to these strings.
    strings: dict[str, str],

    # BUCK1 BACKCOMPAT ONLY!
    #
    # Currently always empty. Will be filled in if a flag is passed in the future.
    #
    # A map from targets with potential `?modifier` modifiers that failed to build to error messages describing the
    # failure.
    failures: dict[TargetLabelWithModifiers, str],

    # Build metrics aggregated across all targets.
    build_metrics: AllTargetsBuildMetrics,

    # Set sketch of configured target graph stored in a hex string.
    # Enabled by setting `-c buck2.log_total_configured_graph_sketch=true`.
    total_configured_graph_sketch: Optional[str],
}

BuildReportEntry {
    # The results of building the target in the given configurations
    configured: dict[Configuration, ConfiguredBuildReportEntry],

    # Errors encountered while building this target.
    #
    # Note that this does not include the errors that are found within the
    # `ConfiguredBuildReportEntry`s. Instead, it includes additional errors
    # which could not be associated with a specific configuration of the
    # target, typically because they occurred before the target could be
    # configured.
    errors: list[Error],

    # BUCK1 BACKCOMPAT ONLY!
    #
    # The two fields below are included for buck1 backwards compatibility only.
    # They are both computed by aggregating across all the configured targets in
    # the way you might expect.
    #
    # `IN_PROGRESS` only appears in a streaming build report. See "Streaming
    # build report" below.
    success: "FAIL" | "SUCCESS" | "IN_PROGRESS",
    outputs: dict[str, list[Path]],

    # The path to the package containing this target, relative to the project
    # root. This is the source code location for this target.
    package_project_relative_path: Optional[str]
}

ConfiguredBuildReportEntry {
    # Did this target build successfully or not?
    #
    # `IN_PROGRESS` only appears in a streaming build report. See "Streaming
    # build report" below.
    success: "FAIL" | "SUCCESS" | "IN_PROGRESS",

    # A map of subtargets that were built to a list of the successfully built
    # outputs for that subtarget.
    #
    # The keys are generated by joining the subtargets with a `|`. For example,
    # if you request to have `:target` and `:target[foo][bar]` built on the CLI,
    # this list will contain one entry for `""` and one for `"foo|bar"`.
    outputs: dict[str, list[Path]],

    # The number of targets in the configured dependency graph of this target.
    #
    # This is only included if `-c buck2.log_configured_graph_size=true` is set.
    # Otherwise, it is left as None.
    configured_graph_size: Optional[uint],

    # Information about this particular artifact. Includes things like its hash, whether it is
    # executable, etc.
    artifact_info: dict[str, ArtifactInfoFile | ArtifactInfoSymlink | ArtifactInfoExternalSymlink],

    # Set sketch of configured target graph stored in a hex string.
    # Enabled by setting `-c buck2.log_configured_graph_sketch=true`.
    configured_graph_sketch: Optional[str],

    # Set sketch of retained analysis memory utilization stored in a hex string.
    #
    # Computing the cardinality of this sketch returns an (approximate) number of bytes.
    #
    # Enabled by setting `-c buck2.log_retained_analysis_memory_sketch=true`
    retained_analysis_memory_sketch: Optional[str],

    # Set sketch of peak analysis memory utilization stored in a hex string.
    #
    # Computing the cardinality of this sketch returns an (approximate) number of bytes.
    #
    # Enabled by setting `-c buck2.log_peak_analysis_memory_sketch=true`
    peak_analysis_memory_sketch: Optional[str],

    # Set sketch of distinct artifact paths stored in a hex string.
    #
    # Computing the cardinality of this sketch returns an (approximate) count
    # of distinct artifact file paths.
    #
    # Enabled by setting `-c buck2.log_artifact_count_sketch=true`.
    artifact_count_sketch: Optional[str],

    # Set sketch of artifact path sizes stored in a hex string. Each path is
    # weighted by its file size in bytes; computing the cardinality returns an
    # (approximate) total artifact size in bytes.
    #
    # Enabled by setting `-c buck2.log_artifact_size_sketch=true`.
    artifact_size_sketch: Optional[str],

    # Estimated cardinality of `artifact_count_sketch`. Populated only when
    # `-c buck2.log_sketch_cardinalities=true` is set; the corresponding sketch
    # field is left intact in both cases.
    artifact_count_sketch_cardinality: Optional[float],

    # Estimated cardinality of `artifact_size_sketch`. Populated only when
    # `-c buck2.log_sketch_cardinalities=true` is set; the corresponding sketch
    # field is left intact in both cases.
    artifact_size_sketch_cardinality: Optional[float],

    # Build metrics for this target. Represents the aggregated metrics for
    # top-level targets. Omitted from the JSON when None.
    build_metrics: Optional[TargetBuildMetrics],

    # Metrics for this target. Represents the aggregated metrics for top level targets.
    metrics: TargetBuildMetrics,
}

AllTargetsBuildMetrics {
    #  The total number of nodes in the action graph, if we were able to fully traverse it.
    action_graph_size: Optional[u64],

    # Metrics aggregated across all targets.
    metrics: AggregatedBuildMetrics,
}

TargetBuildMetrics {
    # The total number of nodes in the action graph, if we were able to fully traverse it.
    action_graph_size: Optional[u64],

    # These are metrics aggregated without normalization.
    metrics: AggregatedBuildMetrics

    # "Amortized" metrics are aggregated by dividing the metric/cost evenly
    # across all top-level targets that require the node that produced the
    # metric. For example, when building four targets `//:foo`, `//:bar`,
    # `//:baz`, `//:qux` if some intermediate action is required for each of the
    # first three, its costs will be aggregated to them each multiplied by 1/3
    # while no cost will be attributed to `//:qux`.
    amortized_metrics: AggregatedBuildMetrics

    # Max value for peak memory usage across all remote actions.
    remote_max_memory_peak_bytes: Optional[u64]

    # Max value for peak memory usage across all local actions.
    local_max_memory_peak_bytes: Optional[u64]

    # The distinct set of RE platform identifiers used by actions for this
    # target. Each value is the worker's `platform` property, optionally
    # suffixed with `.subplatform` (e.g. "linux-remote-execution",
    # "gpu-remote-execution.H100"). Omitted when empty (no remote actions).
    re_platform_names: Optional[List[str]]

    # Wall-clock time in milliseconds from the start of the build at which
    # this top-level target succeeded, failed, or timed out.
    # Use this to compare durations but don't add it to a start timestamp
    # to produce an end timestamp.
    wall_clock_completion_ms: Optional[u64]
}

AggregatedBuildMetrics {
    full_graph_execution_time_ms: float
    full_graph_output_size_bytes: float

    local_execution_time_ms: float
    remote_execution_time_ms: float

    local_executions: float
    remote_executions: float
    remote_cache_hits: float

    analysis_retained_memory: float
    declared_actions: float
}

Error {
    # The stringified hash of the same stringified error message that is shown to the user on the
    # console. The hash is stored as the key in the `strings` cache of the `BuildReport`
    message_content: str,

    # Structured action error. Present only if the error was actually an action error
    action_error: Optional[ActionError],

    # An index that can be used to detect duplicate errors. Two errors with the
    # same cause index have the same cause. Note that that does not mean that
    # they have the same error message.
    cause_index: uint,

    # List of error tags associated with the error. The error tags provide hints to the error category
    # that the error is associated to as determined by Buck2 internally. This is meant to classify errors
    # more precisely, helping developers better understand the nature of the error.
    error_tags: list[str],


    # Error category for the error. One of "USER", "INFRA", "ENVIRONMENT".
    error_category: str,

    # A finer-grained key identifying the category of this error, derived from the
    # error's tags and source location. Matches the `category_key` buck2 logs to
    # its error telemetry, so it can be used to join build report errors with it.
    category_key: str,
}

ActionError {
    # The action key
    key: ActionKey,

    # The action name
    name: ActionName,

    # Digest of the action
    digest: str,

    # Stringified hash of the same stringified error message that is provided by the action
    error_content: str,

    # Stringified hash of the stderr of the action
    stderr_content: str,

    # Stringified hash of the stdout of the action
    stdout_content: str,

    # Optional list of error categorizations provided by an error handler which is invoked
    # in the event of a failed action, or an error message if the error handler failed.
    error_diagnostics: Optional[ActionErrorDiagnostics],
}

ActionKey {
    # The configured target, anon target, or bxl function which owns this action
    owner: str,
}

ActionName {
    # The category of the action
    category: str,

    # The optional identifier of the action
    identifier: Optional[str],
}

enum ActionErrorDiagnostics {
    # The list of sub errors if the error handler succeeded
    sub_errors: list[ActionSubError],

    # The stringified hash of the error message if the error handler failed
    handler_invocation_error: String,
}

ActionSubError {
    # Name of the error category. The category should be finer grain error categorizations
    # provided by the rule authors, and tend to be language specific. These should not be
    # any kind of shared concepts among all errors for all languages/rules. For example,
    # timeouts and infra errors should not go here - buck2 tries to categorize these types
    # of errors automatically. An example of a finer grain error category may be the error
    # code for rustc outputs.
    category: str,

    # The stringified hash of the extra message provided for the specific sub-error category.
    message_content: Optional[str],

    # File path where the error appeared, preferably either project-relative or absolute.
    file: Optional[str],

    # Line number
    lnum: Optional[u64],

    # End line (for multi-line spans)
    end_lnum: Optional[u64],

    # Column number
    col: Optional[u64],

    # End column (for ranges)
    end_col: Optional[u64],

    # Type of error (error, warning, info, etc.)
    error_type: Optional[str],

    # Numeric error code (e.g., 404, 500)
    error_number: Optional[u64],

    # Subcategory for finer-grained categorization
    subcategory: Optional[str],

    # Remediation steps for the error
    remediation: Optional[str],
}

ArtifactInfoFile {
    # The type of this artifact info. This will always be "file".
    kind: str,

    # CAS digest for this file, includes hash and file size.
    digest: str,
    # Whether this file is executable.
    is_exec: bool,
}

ArtifactInfoSymlink {
    # The type of this artifact info. This will always be "symlink".
    kind: str,

    # Symlink target path relative to the root directory.
    symlink_rel_path: str
}

ArtifactInfoExternalSymlink {
    # The type of this artifact info. This will always be "external_symlink".
    kind: str,

    # Symlink target path.
    target: str,
    # An optional trailing path component. Join this with the `target`
    field to get the full symlink file path.
    remaining_path: Optional[str],
}
```

### On Compatibility

The format of the build report is generally stable. However, note that new
fields may be added at any time, and you should ensure this does not cause your
parsing to fail.

A number of fields above are marked as being for buck1 backwards compatibility
only. These fields all have superior alternatives available in the build report
already. We would strongly prefer that new code neither use nor parse them, as
this increases the likelihood that they can be removed one day.

The build report additionally outputs a few fields that are intentionally not
documented here. Those fields are even less useful than ones documented as being
for backwards compatibility only, and even closer to removal. **Please** avoid
using or parsing these if at all possible.

### Limitations

The build report currently has at least the following limitations:

1.  It includes only one action error per failed target. This is the expected
    behavior when `--keep-going` is not passed, but when `--keep-going` is
    passed, this is a bug.
1.  It is currently not generated when a non-existent package is specified on
    the command line. This is also a bug.
1.  The "failures" field is empty by default. To populate it, pass the
    `fill-out-failures` option via `--build-report-options` (this exists for
    buck1 backwards compatibility only; new code should not rely on it).

Finally, it's worth raising that the concept of error deduplication has some
fundamental limitations; if two targets both refer to the same non-existent
dependency, do those errors have the same cause (the dependency doesn't exist)
or different causes (each target is individually broken)? As a result, the exact
details of when two errors are considered to have the same cause are not
generally stable, and may not always be what you expect.

## Streaming build report

Pass `--streaming-build-report <path>` to have buck write build reports to
`<path>` while the build runs, one JSON object per line, in the schema above.

- A line is written each time a requested output is built or fails to build,
  each time a configured target finishes building, and each time a requested
  target does. This includes targets with no outputs and targets that fail
  before they are configured.
- Each line carries one target. A target that is requested with several sets of
  `?modifier` modifiers can have an entry for each of them on the line.
- `success` reads `IN_PROGRESS` on an entry, and on a configured entry, until it
  has finished building. `IN_PROGRESS` only replaces `SUCCESS`: an entry reads
  `FAIL` as soon as an error is recorded for it, which can be before it
  finishes. The final `--build-report` never contains `IN_PROGRESS`.
- With `--fail-fast`, the error that stops the build is on a line for its
  target. If other targets were still building, that line is the last one, and
  their entries can still read `IN_PROGRESS` on their last lines. The same goes
  for the remaining entries when the build is cancelled.
- With `-c build_report.print_unconfigured_section=false`, entries have no
  top-level `success`. Entries that hold only errors, or whose configurations
  were all skipped as incompatible, then have no `success` at all.
- When one target is requested with two sets of modifiers that resolve to the
  same configuration, as in `//:a?macos+arm //:a[sub]?arm+macos`, each entry's
  configured entry finishes when that entry's own request has finished building
  it, whatever the other request is doing. `buck test` differs when the two
  requests also name the same provider label, as in
  `//:a?macos+arm //:a?arm+macos`: it builds that label once, and only one of
  the two entries appears on streaming lines. The other is only in the final
  build report.
- `buck test` writes a line for one provider label at a time, and the line
  carries only that label's build. A target that is tested under several
  provider labels is therefore reported finished once per label, the first time
  possibly while another of its labels is still building.
- Errors that stop `buck test` from building or testing a provider label, such
  as analysis errors, are written on lines of their own. Each of these lines
  carries one target's errors and no builds.
- In `buck test`, an entry that has finished building can still read `FAIL` on
  a later line, if setting up its test fails.
- In `buck test`, treat `FAIL` as final across lines: a later line for another
  provider label of the same target can read `SUCCESS`. An error that stops one
  `?modifier` request before it is configured is written under the target
  without modifiers, and the other request's entry can end the stream reading
  `SUCCESS`. The final build report files both correctly.

### Lines from older buck2 versions

No field on a line says which buck2 wrote it. On a buck2 that predates the
behavior above, `SUCCESS` only means that no error has been recorded yet, not
that the target has finished building, and a line can carry every target seen
so far rather than one. A target with no outputs, or one that fails before
building any output, may not appear on any line. Readers that can get lines
from such a buck2 and need a final verdict before buck exits should therefore
treat only `FAIL` as final, or use the final `--build-report`.
