%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(escript_builder).
-author("loscher@fb.com").
-moduledoc """
Build an escript from a given spec file. The spec file format
is defined in erlang_escript.bzl

usage:
  escript_builder.escript escript_build_spec.term
""".
-compile(warn_missing_spec_all).

-export([main/1]).

-include_lib("kernel/include/file.hrl").

-type escript_artifact_spec() :: #{
    ArchivePath :: file:filename_all() => FileSystemPath :: file:filename_all()
}.
-type escript_load_spec() :: [{ArchivePath :: file:filename(), FileSystemPath :: file:filename()}].
-type escript_archive_spec() :: [{ArchivePath :: file:filename(), binary()}].

-spec main([string()]) -> ok.
main([Spec]) ->
    try
        {ok, Contents} = file:read_file(Spec, [raw]),
        Decoded = json:decode(Contents),
        do(Decoded)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~ts: ~ts~n", [Type, Reason]),
            erlang:halt(1)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("escript_builder.escript build_spec.term ~n").

-spec do(map()) -> ok.
do(#{
    <<"artifacts">> := Artifacts,
    <<"emu_args">> := EmuArgs0,
    <<"output">> := EscriptPath
}) ->
    ArchiveSpec = prepare_files(Artifacts),
    Shebang = "/usr/bin/env escript",
    Comment = "",
    EmuArgs1 = [string:trim(Arg) || Arg <- EmuArgs0],
    FinalEmuArgs = unicode:characters_to_list(
        [" ", lists:join(" ", EmuArgs1)]
    ),
    EscriptSections =
        [
            {shebang, Shebang},
            {comment, Comment},
            {emu_args, FinalEmuArgs},
            {archive, ArchiveSpec, []}
        ],

    case escript:create(EscriptPath, EscriptSections) of
        ok ->
            ok;
        {error, EscriptError} ->
            error(unicode:characters_to_binary(io_lib:format("could not create escript: ~tp", [EscriptError])))
    end,

    %% set executable bits (unix only)
    {ok, #file_info{mode = Mode}} = file:read_file_info(EscriptPath),
    ok = file:change_mode(EscriptPath, Mode bor 8#00111).

-spec prepare_files(escript_artifact_spec()) -> escript_archive_spec().
prepare_files(Artifacts) ->
    Files = expand_to_files_list(Artifacts),
    load_parallel(Files).

-spec expand_to_files_list(escript_artifact_spec()) -> escript_load_spec().
expand_to_files_list(Artifacts) ->
    maps:fold(
        fun(ArchivePathBin, FSPath, AccOuter) ->
            ArchivePath = binary_to_list(ArchivePathBin),
            case filelib:is_dir(FSPath, prim_file) of
                true ->
                    Files = filelib:wildcard("**", binary_to_list(FSPath), prim_file),
                    lists:foldl(
                        fun(FileShortPath, AccInner) ->
                            FileOrDirPath = filename:join(FSPath, FileShortPath),
                            case filelib:is_dir(FileOrDirPath, prim_file) of
                                true ->
                                    AccInner;
                                false ->
                                    [
                                        {filename:join(ArchivePath, FileShortPath), FileOrDirPath}
                                        | AccInner
                                    ]
                            end
                        end,
                        AccOuter,
                        Files
                    );
                false ->
                    [{ArchivePath, FSPath} | AccOuter]
            end
        end,
        [],
        Artifacts
    ).

-spec load_parallel(escript_load_spec()) -> escript_archive_spec().
load_parallel([]) ->
    [];
load_parallel(Files) ->
    Self = self(),
    F = fun() -> worker(Self) end,
    Jobs = min(length(Files), erlang:system_info(schedulers)),
    Refs = #{element(2, spawn_monitor(F)) => [] || _I <- lists:seq(1, Jobs)},
    queue(Files, Refs, maps:size(Refs), []).

-spec worker(pid()) -> ok.
worker(QueuePid) ->
    QueuePid ! self(),
    receive
        {load, {ArchivePath, FSPath}} ->
            QueuePid ! {done, FSPath, {ArchivePath, file_contents(FSPath)}},
            worker(QueuePid);
        empty ->
            ok
    end.

-spec file_contents(file:filename()) -> binary().
file_contents(Filename) ->
    case file:read_file(Filename, [raw]) of
        {ok, Bin} -> Bin;
        Error -> error({read_file, Filename, Error})
    end.

-spec queue(escript_load_spec(), #{reference() => []}, non_neg_integer(), escript_archive_spec()) ->
    escript_archive_spec().
queue([], _JobRefs, 0, Acc) ->
    Acc;
queue(Files, JobRefs, NumLeft, Acc) ->
    receive
        {done, File, Res} ->
            io:format("Loaded ~ts~n", [File]),
            queue(Files, JobRefs, NumLeft, [Res | Acc]);
        {'DOWN', Mref, _, _Pid, Info} ->
            case Info of
                normal when is_map_key(Mref, JobRefs) ->
                    queue(Files, JobRefs, NumLeft - 1, Acc);
                _ ->
                    io:format("ERROR: Compilation failed: ~tp", [Info]),
                    erlang:halt(1)
            end;
        Worker when is_pid(Worker) ->
            case Files of
                [] ->
                    Worker ! empty,
                    queue(Files, JobRefs, NumLeft, Acc);
                [_ | _] ->
                    [NextFile | MoreFiles] = Files,
                    Worker ! {load, NextFile},
                    queue(MoreFiles, JobRefs, NumLeft, Acc)
            end
    end.
