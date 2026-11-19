%% @format
%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(dependency_analyzer).
-author("loscher@fb.com").
-moduledoc """
 Extract direct dependencies from a given erl or hrl file

 usage:
   dependency_analyzer.escript some_file.(h|e)rl [out.term]

 The output of the tool is written either to stdout,
 or a given output file. The format is as follows and intended to
 be consumed by other binary_to_term/1:
```
 [#{type := include
            | include_lib
            | behaviour
            | parse_transform
            | manual_dependency
            | doc_file,
  file  := <<"header_or_source_file.(h|e)rl">>,
  app   => application %% only for include_lib
  },
  ...
 ].
```
""".
-compile(warn_missing_spec_all).

-export([main/1]).

-type entry() ::
    #{type := include, file := binary()}
    | #{type := include_lib, file := binary(), app := binary()}
    | #{type := behaviour, file := binary()}
    | #{type := parse_transform, file := binary()}
    | #{type := manual_dependency, file := binary()}
    | #{type := doc_file, file := binary()}.

-spec main([string()]) -> ok.
main([InFile]) ->
    do(InFile, stdout);
main([InFile, OutFile]) ->
    do(InFile, {file, OutFile});
main(_) ->
    usage(),
    erlang:halt(1).

-spec usage() -> ok.
usage() ->
    io:format("dependency_analyzer.escript some_file.(h|e)rl [out.term]").

-spec do(InFile, OutSpec) -> ok when
    InFile :: file:filename(),
    OutSpec :: {file, file:filename()} | stdout.
do(InFile0, OutSpec) ->
    InFile = dependency_utils:chars_to_binary(InFile0),
    {ok, File} = file:open(InFile, [read, raw, binary, read_ahead]),
    try
        Dependencies = lists:sort(scan_file(File, [], [], 1, [])),
        case OutSpec of
            {file, OutFile0} ->
                OutData = erlang:term_to_binary(Dependencies, [deterministic]),
                OutFile = dependency_utils:chars_to_binary(OutFile0),
                ok = prim_file:write_file(OutFile, OutData);
            stdout ->
                io:format("~tp~n", [Dependencies])
        end
    after
        ok = file:close(File)
    end.

-spec scan_file(File, Cont, Chars, Loc, Acc) -> [entry()] when
    File :: file:io_device(),
    Cont :: erl_scan:return_cont(),
    Chars :: string(),
    Loc :: erl_anno:location(),
    Acc :: [entry()].
scan_file(File, Cont, Chars, Loc, Acc) ->
    case erl_scan:tokens(Cont, Chars, Loc) of
        {done, {ok, Tokens, NextLoc}, Rest} ->
            scan_file(File, [], Rest, NextLoc, extract_dependencies(Tokens) ++ Acc);
        {done, {error, _ErrInfo, NextLoc}, Rest} ->
            scan_file(File, [], Rest, NextLoc, Acc);
        {done, {eof, _NextLoc}, _Rest} ->
            Acc;
        {more, NewCont} ->
            case file:read_line(File) of
                {ok, Bin} ->
                    scan_file(File, NewCont, to_charlist(Bin), Loc, Acc);
                eof ->
                    case erl_scan:tokens(NewCont, eof, Loc) of
                        {done, {ok, Tokens, _}, _} -> extract_dependencies(Tokens) ++ Acc;
                        _ -> Acc
                    end
            end
    end.

-spec extract_dependencies(Tokens) -> [entry()] when
    Tokens :: [erl_scan:token()].
extract_dependencies([{'-', _}, {atom, _, Name} | Rest]) when
    Name =:= include orelse
        Name =:= include_lib orelse
        Name =:= behaviour orelse
        Name =:= behavior orelse
        Name =:= doc orelse
        Name =:= moduledoc orelse
        Name =:= compile orelse
        Name =:= build_dependencies
->
    case parse_attribute_value(Name, Rest) of
        {include, MaybePath} when is_list(MaybePath) ->
            [#{type => include, file => filename:basename(F)} || {ok, F} <- [try_characters_to_binary(MaybePath)]];
        {include_lib, MaybePath} when is_list(MaybePath) ->
            case try_characters_to_binary(MaybePath) of
                {ok, Path} ->
                    case filename:split(Path) of
                        [App, <<"include">>, Include] -> [#{type => include_lib, app => App, file => Include}];
                        _ -> error(malformed_header_include_lib)
                    end;
                error ->
                    []
            end;
        {behaviour, M} when is_atom(M) ->
            [#{type => behaviour, file => module_to_erl(M)}];
        {behavior, M} when is_atom(M) ->
            [#{type => behaviour, file => module_to_erl(M)}];
        {doc, {file, MaybePath}} when is_list(MaybePath) ->
            [#{type => doc_file, file => F} || {ok, F} <- [try_characters_to_binary(MaybePath)]];
        {moduledoc, {file, MaybePath}} when is_list(MaybePath) ->
            [#{type => doc_file, file => F} || {ok, F} <- [try_characters_to_binary(MaybePath)]];
        {compile, Term} ->
            [#{type => parse_transform, file => module_to_erl(M)} || M <- collect_parse_transforms(Term)];
        {build_dependencies, Modules} when is_list(Modules) ->
            [#{type => manual_dependency, file => module_to_erl(M)} || M <- Modules, is_atom(M)];
        _ ->
            []
    end;
extract_dependencies(_) ->
    [].

-spec parse_attribute_value(Name, Tokens) -> {Name, Value} | error when
    Name :: atom(),
    Tokens :: [erl_scan:token()],
    Value :: term().
parse_attribute_value(Name, Tokens) ->
    case erl_parse:parse_term(Tokens) of
        {ok, Term} -> {Name, Term};
        {error, _} -> error
    end.

-spec module_to_erl(Module) -> binary() when
    Module :: module().
module_to_erl(Module) ->
    <<(atom_to_binary(Module))/binary, ".erl"/utf8>>.

-spec to_charlist(Bin) -> string() when
    Bin :: binary().
to_charlist(Bin) ->
    case unicode:characters_to_list(Bin) of
        L when is_list(L) -> L;
        _ -> binary_to_list(Bin)
    end.

-spec try_characters_to_binary(Value) -> {ok, binary()} | error when
    Value :: term().
try_characters_to_binary(Value) ->
    try unicode:characters_to_binary(Value) of
        B when is_binary(B) -> {ok, B};
        _ -> error
    catch
        _:_ -> error
    end.

-spec collect_parse_transforms(Term) -> [module()] when
    Term :: term().
collect_parse_transforms({parse_transform, M}) when is_atom(M) ->
    [M];
collect_parse_transforms(L) when is_list(L) ->
    [M || {parse_transform, M} <- L, is_atom(M)];
collect_parse_transforms(_) ->
    [].
