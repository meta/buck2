%%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%%
%%% This source code is licensed under both the MIT license found in the
%%% LICENSE-MIT file in the root directory of this source tree and the Apache
%%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%%% of this source tree.

%% @format
-module(dependency_finalizer).
-author("loscher@meta.com").

-export([main/1]).

%% Layers of application -> merged dep file, outermost first. The keys are unused;
%% the layering exists so the caller can pass its own entries alongside the ones it
%% inherits without merging maps sized by its transitive closure.
-type dep_files_data() :: [#{filename() => filename()}].

-type filename() :: binary().

-spec main([string()]) -> ok | no_return().
main([Source, InFile]) ->
    do(Source, InFile, stdout);
main([Source, InFile, OutFile]) ->
    do(Source, InFile, {file, OutFile});
main(_) ->
    usage(),
    erlang:halt(1).

-spec usage() -> ok.
usage() ->
    io:format("~ts.escript source_dep_file.dep dependency_spec.term [out.json]", [?MODULE]).

-spec do(file:filename(), file:filename(), {file, file:filename()} | stdout) -> ok.
do(Source0, InFile0, OutSpec) ->
    Source = dependency_utils:chars_to_binary(Source0),
    InFile = dependency_utils:chars_to_binary(InFile0),
    try
        case read_file(InFile) of
            {ok, DepFileLayers} ->
                FlatDepFiles = lists:foldl(
                    fun(Layer, Acc) -> maps:fold(fun merge_deps/3, Acc, Layer) end, #{}, DepFileLayers
                ),
                Dependencies = build_dep_info(Source, FlatDepFiles),
                OutData = json:encode(Dependencies),
                case OutSpec of
                    {file, File0} ->
                        ok = prim_file:write_file(dependency_utils:chars_to_binary(File0), OutData);
                    stdout ->
                        io:format("~ts~n", [OutData])
                end;
            Err ->
                io:format(standard_error, "error, could no parse file correctly: ~tp~n", [Err]),
                erlang:halt(1)
        end
    catch
        Class:Reason:Stack ->
            io:format(standard_error, "~ts", [erl_error:format_exception(Class, Reason, Stack)]),
            erlang:halt(1)
    end.

merge_deps(_File, DepFile, Acc) ->
    case read_file_term(DepFile) of
        {ok, Dependencies} ->
            maps:merge(Dependencies, Acc);
        Err ->
            io:format(standard_error, "error, could no parse file correctly: ~tp~n", [Err]),
            erlang:halt(1)
    end.

-spec read_file(filename()) -> {ok, dep_files_data()} | {error, term()}.
read_file(File) ->
    case prim_file:read_file(File) of
        {ok, Data} ->
            {ok, json:decode(Data)};
        Err ->
            Err
    end.

-spec read_file_term(filename()) -> {ok, dep_files_data()} | {error, term()}.
read_file_term(File) ->
    case prim_file:read_file(File) of
        {ok, Data} ->
            {ok, erlang:binary_to_term(Data)};
        Err ->
            Err
    end.

-spec build_dep_info(filename(), dep_files_data()) -> list(map()).
build_dep_info(Source, DepFiles) ->
    case read_file_term(Source) of
        {ok, Dependencies} ->
            Basename = filename:basename(Source, ~".dep"),
            Key = <<Basename/binary, ".erl"/utf8>>,
            {NextKeys, NextVisited, NextAcc} = collect_dependencies_for_key(
                Dependencies, Key, [], sets:new([{version, 2}]), []
            ),
            collect_dependencies(NextKeys, DepFiles, NextVisited, NextAcc);
        Err ->
            io:format(standard_error, "error, could no parse file correctly: ~tp~n", [Err]),
            erlang:halt(1)
    end.

collect_dependencies([], _, _, Acc) ->
    Acc;
collect_dependencies([Key | Rest], DepFiles, Visited, Acc) ->
    case DepFiles of
        #{Key := DepBin} ->
            Dependencies = erlang:binary_to_term(DepBin),
            {NextKeys, NextVisited, NextAcc} =
                collect_dependencies_for_key(Dependencies, Key, Rest, Visited, Acc),
            collect_dependencies(
                NextKeys,
                DepFiles,
                NextVisited,
                NextAcc
            );
        _ ->
            %% We cannot find key in DepFiles, which means it's from OTP or missing
            %% We don't add anything to the dependencies and let the compiler fail for a proper error message.
            collect_dependencies(Rest, DepFiles, Visited, Acc)
    end.

collect_dependencies_for_key([], _CurrentKey, KeysAcc, VisitedAcc, DepAcc) ->
    {KeysAcc, VisitedAcc, DepAcc};
collect_dependencies_for_key([#{file := File, type := Type} = Dep | Deps], CurrentKey, KeysAcc, VisitedAcc, DepAcc) ->
    NextKey = File,
    case sets:is_element(NextKey, VisitedAcc) of
        true ->
            collect_dependencies_for_key(Deps, CurrentKey, KeysAcc, VisitedAcc, DepAcc);
        false when Type =:= include; Type =:= include_lib ->
            collect_dependencies_for_key(
                Deps, CurrentKey, [NextKey | KeysAcc], sets:add_element(CurrentKey, VisitedAcc), [Dep | DepAcc]
            );
        false ->
            collect_dependencies_for_key(Deps, CurrentKey, KeysAcc, sets:add_element(CurrentKey, VisitedAcc), [
                Dep | DepAcc
            ])
    end.
