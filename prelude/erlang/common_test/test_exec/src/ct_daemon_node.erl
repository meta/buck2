%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(ct_daemon_node).
-compile(warn_missing_spec_all).

%% Public API
-export([start/1, start/2, stop/0, alive/0, get_node/0]).

-export([node_main/1, get_domain_type/0]).

-export_type([config/0]).

-include_lib("kernel/include/logger.hrl").

-import(common_util, [unicode_characters_to_binary/1, filename_all_to_filename/1]).

-define(LOG_BASE, "/tmp/ct_daemon").

-type config() :: #{
    type := shortnames | longnames,
    name := node(),
    cookie := atom(),
    options := [opt()]
}.

-type opt() ::
    {multiply_timetraps, number() | infinity}
    | {ct_hooks, [atom() | {atom(), [term()]}]}
    | {output_dir, file:filename_all()}.

-doc """
start node for running tests in isolated way and keep state
""".
-spec start(ErlCommand) -> ok | {error, {crash_on_startup, integer()}} when
    ErlCommand :: nonempty_list(binary()).
start(ErlCommand) ->
    NodeName = list_to_atom(
        lists:flatten(io_lib:format("test~ts-atn@localhost", [random_name()]))
    ),
    StartConfig = #{
        type => shortnames,
        name => NodeName,
        cookie => ct_runner:cookie(),
        options => []
    },
    start(ErlCommand, StartConfig).

-doc """
start node for running tests in isolated way and keep state
""".
-spec start(ErlCommand, Config) -> ok | {error, {crash_on_startup, integer()}} when
    ErlCommand :: nonempty_list(binary()),
    Config :: config().
start(
    ErlCommand,
    _Config = #{
        type := Type,
        name := Node,
        cookie := Cookie,
        options := Options
    }
) ->
    RandomName = random_name(),
    ok = ensure_distribution(Type, RandomName, Cookie),
    %% get code paths from current node
    CodePaths = code:get_path(),
    ConfigFiles = get_config_files(),
    OutputDir = gen_output_dir(RandomName),
    FullOptions = [{output_dir, OutputDir} | Options],
    Args = build_daemon_args(Type, Node, Cookie, FullOptions, OutputDir),

    MainProc = self(),
    Pid = erlang:spawn(fun() ->
        true = erlang:register(?MODULE, self()),
        Port = ct_runner:start_test_node(
            ErlCommand,
            [],
            [],
            CodePaths,
            ConfigFiles,
            OutputDir,
            [{args, Args}, {cd, OutputDir}]
        ),
        port_loop_1(Port, MainProc)
    end),

    %% wait for the ct_daemon gen_server to be started
    receive
        {Pid, ready} ->
            ok;
        {Pid, Error = {crash_on_startup, N}} when is_integer(N) ->
            ?LOG_DEBUG("Test Node Crashed on Startup"),
            {error, Error}
    end.

-spec port_loop_1(port(), pid()) -> ok.
port_loop_1(Port, MainProc) ->
    receive
        ready ->
            true = erlang:unregister(?MODULE),
            ok = global:sync(),
            MainProc ! {self(), ready},
            port_loop_2(Port);
        {Port, {exit_status, N}} ->
            MainProc ! {self(), {crash_on_startup, N}},
            ok;
        {Port, {data, {noeol, Line}}} ->
            io:put_chars(Line),
            port_loop_1(Port, MainProc);
        {Port, {data, {eol, Line}}} ->
            io:put_chars([Line, ~"\n"]),
            port_loop_1(Port, MainProc)
    end.

-spec port_loop_2(port()) -> ok.
port_loop_2(Port) ->
    receive
        {Port, {exit_status, _}} ->
            ok;
        {Port, {data, {noeol, Line}}} ->
            io:put_chars(Line),
            port_loop_2(Port);
        {Port, {data, {eol, Line}}} ->
            io:put_chars([Line, ~"\n"]),
            port_loop_2(Port)
    end.

-spec stop() -> node().
stop() ->
    %% the gen_server might be blocked, we spawn a process on the
    %% remote note that exectues `erlang:halt()' to bypass

    Node = get_node(),

    %% monitore node
    true = erlang:monitor_node(Node, true),
    %% kill node
    %% elp:ignore W0014
    _Pid = erlang:spawn(Node, fun erlang:halt/0),
    %% wait for node to come down
    receive
        {nodedown, Node} -> ok
    end,
    Node.

-spec get_node() -> node().
get_node() ->
    case alive() of
        true ->
            case get_runner_pid() of
                undefined -> error(not_running);
                Pid -> erlang:node(Pid)
            end;
        false ->
            error(not_running)
    end.

-spec alive() -> boolean().
alive() ->
    erlang:is_pid(get_runner_pid()).

-doc """
node main entry point
""".
-spec node_main([node()]) -> no_return().
node_main([Parent, OutputDirAtom]) ->
    ok = application:load(test_exec),
    OutputDir = erlang:atom_to_list(OutputDirAtom),

    %% set stack trace to 20
    erlang:system_flag(backtrace_depth, 20),

    %% setup logger and prepare IO
    ok = ct_daemon_logger:start(OutputDir),

    %% setup capture-aware group leader to support ct:capture_* API
    ok = setup_capture_group_leader(),

    true = net_kernel:connect_node(Parent),

    {ok, {RunnerPid, RunnerMonRef}} = ct_daemon_runner:start_monitor(Parent, OutputDir),
    {ok, {HooksPid, HooksMonRef}} = ct_daemon_hooks:start_monitor(),

    true = erlang:monitor_node(Parent, true),
    {?MODULE, Parent} ! ready,

    %% block unless parent node dies or ct_daemon_runner
    receive
        {nodedown, _} ->
            ?LOG_INFO("parent node went down, terminating test node", []),
            ok;
        {'DOWN', RunnerMonRef, process, RunnerPid, _} ->
            ?LOG_INFO("ct_daemon_runner went down, terminating test node", []),
            ok;
        {'DOWN', HooksMonRef, process, HooksPid, _} ->
            ?LOG_INFO("ct_daemon_hooks went down, terminating test node", []),
            ok
    end,
    test_logger:flush(),
    erlang:halt(0).

%% internal
-spec ensure_distribution(Type, RandomName, Cookie) -> ok when
    Type :: shortnames | longnames,
    RandomName :: binary(),
    Cookie :: atom().
ensure_distribution(Type, RandomName, Cookie) ->
    case erlang:node() of
        'nonode@nohost' ->
            % distribution is not started, ensure epmd is
            (erl_epmd:names("localhost") =:= {error, address}) andalso
                ([] = os:cmd("epmd -daemon")),
            Name = list_to_atom(
                lists:flatten(
                    io_lib:format("ct_daemon~ts@localhost", [RandomName])
                )
            ),
            {ok, _Pid} = net_kernel:start(Name, #{name_domain => Type}),
            ok;
        _ ->
            %% check that the domain is correct
            case get_domain_type() of
                Type -> ok;
                Other -> error({distribution_type_mismatch, Type, Other})
            end
    end,
    true = erlang:set_cookie(Cookie),
    ok.

-spec build_daemon_args(Type, Node, Cookie, Options, OutputDir) ->
    [string()]
when
    Type :: shortnames | longnames,
    Node :: node(),
    Cookie :: atom(),
    Options :: [opt()],
    OutputDir :: file:filename_all().
build_daemon_args(Type, Node, Cookie, Options, OutputDir) ->
    DistArg =
        case Type of
            longnames -> "-name";
            shortnames -> "-sname"
        end,
    [
        DistArg,
        convert_atom_arg(Node),
        "-setcookie",
        convert_atom_arg(Cookie),
        "-test_exec",
        "daemon_options",
        lists:flatten(io_lib:format("~w", [Options])),
        "-s",
        convert_atom_arg(?MODULE),
        "node_main",
        convert_atom_arg(erlang:node()),
        filename_all_to_filename(OutputDir)
    ].

-spec convert_atom_arg(atom()) -> string().
convert_atom_arg(Arg) ->
    lists:flatten(io_lib:format("~ts", [Arg])).

-spec get_config_files() -> [file:filename_all()].
get_config_files() ->
    %% get config files from command line
    case init:get_argument(config) of
        error -> [];
        {ok, Configs} -> [F || ConfigFiles <- Configs, F <- ConfigFiles]
    end.

-spec gen_output_dir(RandomName :: file:filename_all()) -> file:filename_all().
gen_output_dir(RandomName) ->
    BaseDir =
        case application:get_env(test_exec, ct_daemon_log_dir, undefined) of
            undefined ->
                ?LOG_BASE;
            LogDir ->
                LogDir
        end,
    filename:join([
        BaseDir,
        "tests",
        RandomName
    ]).

-spec random_name() -> binary().
random_name() ->
    N = io_lib:format("~b-~b~ts", [rand:uniform(100000), erlang:unique_integer([positive, monotonic]), os:getpid()]),
    unicode_characters_to_binary(N).

-spec get_domain_type() -> longnames | shortnames.
get_domain_type() ->
    case net_kernel:get_state() of
        #{name_domain := shortnames} -> shortnames;
        #{name_domain := longnames} -> longnames
    end.

-spec get_runner_pid() -> pid() | undefined.
get_runner_pid() ->
    global:whereis_name(ct_daemon_runner:name(node())).

-spec setup_capture_group_leader() -> ok.
setup_capture_group_leader() ->
    %% Create a capture-aware group leader that handles {capture, Pid} messages
    %% from ct:capture_start()/test_server:capture_start()
    OriginalGL = erlang:group_leader(),
    case ct_daemon_capture:start_link(OriginalGL) of
        {ok, CaptureGL} ->
            true = erlang:group_leader(CaptureGL, self()),
            ok;
        _ ->
            %% If capture setup fails, continue without it
            ok
    end.
