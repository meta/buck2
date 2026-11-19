%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test_artifact_directory).
-moduledoc """
Artefact directory file management.
Used by TPX to upload diagnostic reports.
""".
-compile(warn_missing_spec_all).

%% Public API
-export([prepare/3, link_to_artifact_dir/3, find_log_private/1]).

-export_type([dir_path/0]).

-include_lib("common/include/buck_ct_records.hrl").
-include_lib("kernel/include/logger.hrl").

-import(common_util, [unicode_characters_to_list/1]).

-define(raw_file_access, prim_file).

-type dir_path() :: file:filename() | undefined.

% Gets the artifactory directory path.
% This one might be undefined if tpx is ran in offline mode.
-spec artifact_dir() -> dir_path().
artifact_dir() ->
    ArtifactDir = os:getenv("TEST_RESULT_ARTIFACTS_DIR"),
    case ArtifactDir of
        false ->
            undefined;
        Dir ->
            filelib:ensure_path(Dir),
            Dir
    end.

-spec with_artifact_dir(fun((file:filename()) -> X)) -> X | ok.
with_artifact_dir(Func) ->
    case artifact_dir() of
        undefined -> ok;
        Dir -> Func(Dir)
    end.

-spec artifact_annotation_dir() -> dir_path().
artifact_annotation_dir() ->
    ArtifactAnnotationDir = os:getenv("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR"),
    case ArtifactAnnotationDir of
        false ->
            undefined;
        Dir ->
            filelib:ensure_path(Dir),
            Dir
    end.

-spec with_artifact_annotation_dir(fun((file:filename()) -> X)) -> X | ok.
with_artifact_annotation_dir(Func) ->
    case artifact_annotation_dir() of
        undefined -> ok;
        Dir -> Func(Dir)
    end.

-spec coverage_tmp_dir() -> file:filename_all() | undefined.
coverage_tmp_dir() ->
    case os:getenv("COVERAGE_COLLECTION_TMPDIR") of
        false ->
            undefined;
        Dir ->
            filename:absname(Dir)
    end.

% Collect, create and link the logs and other relevant files in
% the artefacts directory.
-spec prepare(ExecutionDir, Tests, ArtifactAnnotationFunction) -> ok when
    ExecutionDir :: file:filename_all(),
    Tests :: [#ct_test{}],
    ArtifactAnnotationFunction :: artifact_annotations:annotation_function().
prepare(ExecutionDir, Tests, ArtifactAnnotationFunction) ->
    with_artifact_dir(
        fun(_ArtifactDir) ->
            link_tar_ball(ExecutionDir),
            case os:getenv("TEST_RESULTS_OUTPUT_FILE") of
                false ->
                    ok;
                OutputFile ->
                    link_to_artifact_dir(
                        OutputFile, filename:dirname(OutputFile), fun artifact_annotations:default_annotation/1
                    )
            end,
            case coverage_tmp_dir() of
                undefined ->
                    ok;
                CoverageTmpDir ->
                    link_to_artifact_dir(
                        join_paths(CoverageTmpDir, "feature_coverage.json"),
                        CoverageTmpDir,
                        ArtifactAnnotationFunction
                    )
            end,
            case find_log_private(ExecutionDir) of
                {error, log_private_not_found} ->
                    ok;
                LogPrivate ->
                    LogFiles = find_log_files(LogPrivate),
                    [
                        link_to_artifact_dir(File, LogPrivate, ArtifactAnnotationFunction)
                     || File <- LogFiles,
                        filelib:is_regular(File, ?raw_file_access)
                    ],
                    link_to_artifact_dir(
                        join_paths(LogPrivate, "test.pftrace"), LogPrivate, ArtifactAnnotationFunction
                    ),
                    link_to_artifact_dir(
                        join_paths(LogPrivate, "test_metrics.tcompact.b64"),
                        LogPrivate,
                        fun(FileName) -> artifact_annotations:test_metrics_artifact_annotation(FileName, Tests) end
                    )
            end,
            ok
        end
    ).

-doc """
Use hardlinks inside RE which doesn't follow symlinks like local execution uploads.
""".
-spec make_artifact_link(file:filename_all(), file:filename_all()) ->
    ok | {error, badarg | file:posix()}.
make_artifact_link(File, ArtifactPath) ->
    case os:getenv("INSIDE_RE_WORKER") of
        "1" -> file:make_link(File, ArtifactPath);
        _ -> file:make_symlink(filename:absname(File), ArtifactPath)
    end.

-spec link_to_artifact_dir(File, Root, ArtifactAnnotationMFA) -> ok when
    File :: file:filename_all(),
    Root :: file:filename_all(),
    ArtifactAnnotationMFA :: artifact_annotations:annotation_function().
link_to_artifact_dir(File, Root, ArtifactAnnotationMFA) ->
    with_artifact_dir(
        fun(ArtifactDir) ->
            RelativePath =
                case string:prefix(File, Root) of
                    nomatch ->
                        ?LOG_ERROR("~ts should be a prefix of ~ts", [Root, File]),
                        error(unexpected_path);
                    Suffix ->
                        string:strip(unicode_characters_to_list(Suffix), left, $/)
                end,
            FullFileName =
                unicode_characters_to_list(string:replace(RelativePath, "/", ".", all)),
            case filelib:is_file(File, ?raw_file_access) of
                true ->
                    make_artifact_link(File, join_paths(ArtifactDir, FullFileName)),
                    Annotation = artifact_annotations:create_artifact_annotation(FullFileName, ArtifactAnnotationMFA),
                    dump_annotation(Annotation, FullFileName);
                _ ->
                    ok
            end
        end
    ).

-spec link_tar_ball(file:filename_all()) -> ok.
link_tar_ball(LogDir) ->
    with_artifact_dir(
        fun(ArtifactDir) ->
            {_Pid, MonitorRef} = spawn_monitor(fun() ->
                {ok, Items} = file:list_dir(LogDir),
                Files = [
                    {Item, filename:join(LogDir, Item)}
                 || Item <- Items, not string:equal(filename:join(LogDir, Item), ArtifactDir)
                ],
                erl_tar:create(
                    filename:join(ArtifactDir, "execution_dir.tar.gz"),
                    Files,
                    [compressed]
                )
            end),
            receive
                {'DOWN', MonitorRef, _Type, _Object, _Info} -> ok
            after 15000 -> ok
            end
        end
    ).

-spec dump_annotation(artifact_annotations:test_result_artifact_annotations(), file:filename()) -> ok.
dump_annotation(Annotation, FileName) ->
    with_artifact_annotation_dir(
        fun(ArtifactAnnotationDir) ->
            AnnotationName = FileName ++ ".annotation",
            {ok, AnnotationFile} = file:open(
                filename:join(ArtifactAnnotationDir, AnnotationName), [write, raw]
            ),
            file:write(AnnotationFile, artifact_annotations:serialize(Annotation)),
            file:close(AnnotationFile),
            ok
        end
    ).

-spec find_log_private(file:filename_all()) -> {error, log_private_not_found} | file:filename().
find_log_private(LogDir) ->
    % Use system find command for much faster directory traversal
    % -type d: find directories only
    % -name "log_private": match exact name
    % -print -quit: print first match and exit immediately
    FindCmd = lists:flatten(
        io_lib:format("find '~s' -type d -name \"log_private\" -print -quit 2>/dev/null", [LogDir])
    ),
    case string:trim(os:cmd(FindCmd)) of
        "" -> {error, log_private_not_found};
        Result -> Result
    end.

-spec find_log_files(file:filename()) -> [file:filename()].
find_log_files(LogPrivate) ->
    % Use system find command for much faster directory traversal
    % -type f: find files only
    % -name "*.log": match files ending with .log
    FindCmd = lists:flatten(io_lib:format("find '~s' -type f -name \"*.log\" 2>/dev/null", [LogPrivate])),
    Output = string:trim(os:cmd(FindCmd)),
    case Output of
        "" -> [];
        _ -> string:split(Output, "\n", all)
    end.

-spec join_paths(file:filename_all(), file:filename_all()) -> string().
join_paths(Path, FileName) ->
    case filename:join(Path, FileName) of
        NewPath when is_list(NewPath) -> NewPath
    end.
