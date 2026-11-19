%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(shell_buck2_utils).
-moduledoc """
Documentation for shell_buck2_utils, ways to use
  it, ways to break it, etc. etc
""".

-compile(warn_missing_spec_all).

%% Public API
-export([
    project_root/0,
    cell_root/0,
    rebuild_modules/1,
    buck2_build_targets/1,
    buck2_query/1, buck2_query/2, buck2_query/3,
    run_command/1, run_command/2,
    no_stderr/1,
    get_additional_paths/1,
    get_app_env/1
]).

-import(common_util, [unicode_characters_to_binary/1]).

-type opt() :: {at_root, boolean()}.

-spec project_root() -> file:filename_all().
project_root() ->
    root(project).

-spec cell_root() -> file:filename_all().
cell_root() ->
    root(cell).

-spec root(Type :: cell | project) -> file:filename_all().
root(Type) ->
    case run_command(no_stderr([~"buck2", ~"root", ~"--kind", atom_to_binary(Type)]), [{at_root, false}]) of
        {ok, Output} ->
            Dir = unicode_characters_to_binary(string:trim(Output)),
            case filelib:is_dir(Dir, prim_file) of
                true -> Dir;
                false -> error({project_root_not_found, Dir})
            end;
        error ->
            error(failed_to_query_project_root)
    end.

-spec rebuild_modules([module()]) -> ok | error.
rebuild_modules([]) ->
    ok;
rebuild_modules(Modules) ->
    case lists:filter(fun(Module) -> code:which(Module) == non_existing end, Modules) of
        [] -> ok;
        Missing -> error({non_existing, Missing})
    end,
    RelSources = [proplists:get_value(source, Module:module_info(compile)) || Module <- Modules],
    {ok, RawQueryResult} = buck2_query(~"owner(\%s)", RelSources),
    Targets = string:split(string:trim(RawQueryResult), "\n", all),
    case Targets of
        [[]] ->
            io:format(~"ERROR: couldn't find targets for ~w~n", [Modules]),
            error;
        _ ->
            buck2_build_targets(Targets)
    end.

-spec buck2_build_targets(Targets) -> ok | error when
    Targets :: [string() | binary()].
buck2_build_targets(Targets) ->
    case
        run_command([
            ~"buck2",
            ~"build",
            ~"--reuse-current-config",
            ~"--console=super",
            get_buck2_args_from_env()
            | Targets
        ])
    of
        {ok, _Output} -> ok;
        error -> error
    end.

-spec buck2_query(Query) -> {ok, binary()} | error when
    Query :: string() | binary().
buck2_query(Query) ->
    buck2_query(Query, []).

-spec buck2_query(Query, Args) -> {ok, binary()} | error when
    Query :: string() | binary(),
    Args :: [string() | binary()].
buck2_query(Query, Args) ->
    buck2_query(Query, [], Args).

-spec buck2_query(Query, BuckArgs, Args) -> {ok, binary()} | error when
    Query :: string() | binary(),
    BuckArgs :: [string() | binary()],
    Args :: [string() | binary()].
buck2_query(Query, BuckArgs, Args) ->
    run_command(
        no_stderr([
            ~"buck2",
            ~"uquery",
            ~"--reuse-current-config",
            get_buck2_args_from_env(),
            BuckArgs,
            Query,
            Args
        ])
    ).

-spec no_stderr(CommandArgs) -> CommandArgs when
    CommandArgs :: [unicode:chardata()].
no_stderr(CommandArgs) ->
    [~"sh", ~"-c", ~"exec 2>/dev/null $0 \"$@\"" | CommandArgs].

-spec run_command(CommandArgs) -> {ok, binary()} | error when
    CommandArgs :: [unicode:chardata()].
run_command(Command) ->
    run_command(Command, []).

-spec run_command(CommandArgs, Options) -> {ok, binary()} | error when
    CommandArgs :: unicode:chardata(),
    Options :: [opt()].
run_command(Command, Options) when is_binary(Command) ->
    run_command([Command], Options);
run_command(CmdArgs, Options) when is_list(CmdArgs) ->
    [Cmd | Args] = flatten_command_args(CmdArgs, []),
    CmdPath =
        case os:find_executable(binary_to_list(Cmd)) of
            false -> error({command_not_found, Cmd});
            Path -> Path
        end,
    PortOpts0 = [{args, Args}, exit_status],
    PortOpts1 =
        case proplists:get_value(at_root, Options, true) of
            true ->
                Root = project_root(),
                [{cd, Root} | PortOpts0];
            false ->
                PortOpts0
        end,

    Port = erlang:open_port({spawn_executable, CmdPath}, PortOpts1),
    port_loop(Port, []).

-spec flatten_command_args(CommandArgs, Acc) -> Acc when
    CommandArgs :: iodata(),
    Acc :: [binary()].
flatten_command_args([], Acc) ->
    lists:reverse(Acc);
flatten_command_args([H | T], Acc) when is_binary(H) ->
    flatten_command_args(T, [H | Acc]);
flatten_command_args([H = [I | _] | T], Acc) when is_integer(I) ->
    flatten_command_args(T, [unicode_characters_to_binary(H) | Acc]);
flatten_command_args([[] | T], Acc) ->
    flatten_command_args(T, Acc);
flatten_command_args([[H | T1] | T2], Acc) ->
    flatten_command_args([H, T1 | T2], Acc).

-spec port_loop(port(), [binary()]) -> {ok, binary()} | error.
port_loop(Port, StdOut) ->
    receive
        {Port, {exit_status, 0}} ->
            {ok, unicode_characters_to_binary(lists:reverse(StdOut))};
        {Port, {exit_status, _}} ->
            error;
        {Port, {data, Data}} ->
            port_loop(Port, [Data | StdOut])
    end.

-spec get_additional_paths(file:filename_all()) -> [file:filename_all()].
get_additional_paths(Path) ->
    case
        run_command([
            ~"buck2",
            ~"bxl",
            ~"--reuse-current-config",
            ~"--console=super",
            get_buck2_args_from_env(),
            ~"prelude//erlang/shell/shell.bxl:ebin_paths",
            ~"--",
            ~"--source",
            Path
        ])
    of
        {ok, Output} ->
            MaybeOutputPaths = [
                filter_escape_chars(OutputPath)
             || OutputPath <- string:split(Output, "\n", all)
            ],
            MaybeAllPaths = [
                RelevantPath
             || OutputPath <- MaybeOutputPaths,
                filelib:is_dir(OutputPath, prim_file),
                RelevantPath <- [OutputPath, filename:join(OutputPath, "ebin")]
            ],
            Result =
                [MaybePath || MaybePath <- MaybeAllPaths, filelib:is_dir(MaybePath, prim_file)],
            Result;
        error ->
            []
    end.

%% copied from stackoverflow: https://stackoverflow.com/questions/14693701/how-can-i-remove-the-ansi-escape-sequences-from-a-string-in-python
-define(ANSI_ESCAPE_REGEX,
    ~"(\x9B|\x1B\\[)[0-?]*[ -/]*[@-~]"
).

-spec filter_escape_chars(String) -> binary() when
    String :: binary().
filter_escape_chars(String) ->
    unicode_characters_to_binary(re:replace(String, ?ANSI_ESCAPE_REGEX, "", [global])).

-spec get_buck2_args_from_env() -> [binary()].
get_buck2_args_from_env() ->
    case get_app_env(buck2_args) of
        undefined -> [];
        {ok, Buck2Args} when is_list(Buck2Args) -> Buck2Args
    end.

-spec get_app_env(Key) -> {ok, Value} | undefined when
    Key :: atom(),
    Value :: dynamic().
get_app_env(Key) ->
    _ = application:load(buck2_shell_utils),
    % elp:ignore W0011 (application_get_env)
    application:get_env(buck2_shell_utils, Key).
