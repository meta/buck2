%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test).
-moduledoc """
User-Facing library for quick-iteration testing of Common Test

  use test:help() for more information
""".
-typing([eqwalizer]).
-compile(warn_missing_spec_all).

%% Public API
-export([
    start/0,
    help/0,
    list/0, list/1,
    rerun/1,
    run/0, run/1,
    reset/0,
    logs/0
]).

%% init
-export([
    info/0,
    ensure_initialized/0,
    start_shell/0
]).

%% Test functions
-export([
    list_impl/1
]).

-include_lib("common/include/buck_ct_records.hrl").

-define(raw_file_access, prim_file).

-type test_id() :: string() | non_neg_integer() | atom().
-type test_info() :: #{name := string(), suite := module()}.
-type run_spec() :: test_id() | [test_info()].
-type run_result() :: {non_neg_integer(), non_neg_integer()}.

-type provided_test_info() :: test_info:test_info().

-spec start() -> ok.
start() ->
    info(),
    ensure_initialized().

-spec info() -> ok.
info() ->
    io:format("~n"),
    io:format(
        "------------------------------------ Test Shell ------------------------------------~n"
    ),
    io:format(
        "Test Shell for interactive testing. Within this shell you can run tests, and update ~n"
    ),
    io:format(
        "the code by recompiling and hot-code loading it with c(module). You can also load ~n"
    ),
    io:format("additional modules with l(module). ~n"),
    io:format(
        "The `test` module provides functionality to list, run tests. Check the available  ~n"
    ),
    io:format("functions (test:help()): ~n~n"),
    help(),
    io:format("~n").

-doc """
Print a description of all available commands.
""".
-spec help() -> ok.
help() ->
    io:format("Buck2 Common Test Runner Shell Interface~n~n"),
    [
        print_help(F, A)
     || {F, A} <- ?MODULE:module_info(exports),
        not lists:member(F, [module_info, ensure_initialized, start, start_shell, list_impl])
    ],
    io:format("~n"),
    io:format("For more information, use the built in help, e.g. h(test, help)~n"),
    ok.

-spec print_help(Fun :: atom(), arity()) -> ok.
print_help(Fun, Arity) ->
    #{args := Args, desc := [DescFirst | DescRest]} = command_description(Fun, Arity),
    FunSig = string:pad(
        io_lib:format("~ts:~ts(~ts)", [?MODULE, Fun, lists:join(", ", Args)]), 30, trailing
    ),
    io:format("~ts -- ~ts~n", [FunSig, DescFirst]),
    Padding = string:pad("", 34),
    [io:format("~ts~ts~n", [Padding, DescLine]) || DescLine <- DescRest],
    ok.

-spec command_description(Fun :: atom(), arity()) -> #{args := [string()], desc := [string()]}.
command_description(help, 0) ->
    #{args => [], desc => ["print help"]};
command_description(info, 0) ->
    #{args => [], desc => ["print info text"]};
command_description(list, 0) ->
    #{args => [], desc => ["list all available tests"]};
command_description(list, 1) ->
    #{
        args => ["RegExOrModule"],
        desc => ["same as list(), but filter tests with RegEx, or a test suite"]
    };
command_description(list, 2) ->
    #{
        args => ["Module", "RegEx"],
        desc => ["same as list(), but filter tests with RegEx, and a test suite"]
    };
command_description(rerun, 1) ->
    #{
        args => ["IdOrRegex"],
        desc =>
            [
                "runs a test with the shortest possible setup path, the test can ",
                "be given as RegEx matching a single test, or the id from listing. ",
                "This command does *not* recompile the test suite or its dependencies"
            ]
    };
command_description(run, 0) ->
    #{args => [], desc => ["run all tests"]};
command_description(run, 1) ->
    #{
        args => ["IdOrRegex"],
        desc =>
            [
                "same as rerun/1 but does compile the targeted suite, and loads",
                "changed modules in the remote node."
            ]
    };
command_description(reset, 0) ->
    #{args => [], desc => ["restarts the test node, enabling a clean test state"]};
command_description(logs, 0) ->
    #{args => [], desc => ["print log files of the currently running test suites"]};
command_description(F, A) ->
    error({help_is_missing, {F, A}}).

-doc """
List all available tests
""".
-doc #{equiv => test:list("")}.
-spec list() -> ok | {error, term()}.
list() ->
    list("").

-doc """
List all available tests, filters by the given RegEx. Please check
[https://www.erlang.org/doc/man/re.html#regexp_syntax] for the supported
regular expression syntax. If a module is given as argument, list all
tests from that module instead
""".
-spec list(RegExOrModule :: module() | string()) -> ok | {error, term()}.
list(RegEx) when is_list(RegEx) ->
    case list_impl(RegEx) of
        {ok, TestsString} -> io:format("~ts", [TestsString]);
        Error -> Error
    end.

-doc """
Run a test given by either the test id from the last list() command, or
a regex that matches exactly one test. Tests are run with the shortest possible
setup. This call does not recompile the test suite and its dependencies, but
runs them as is. You can manually recompile code with c(Module).
To reset the test state use reset().
""".
-spec rerun(run_spec()) -> run_result().
rerun(Spec) ->
    ensure_initialized(),
    do_plain_test_run(Spec).

-doc """
update code and run all tests
""".
-doc #{equiv => run("")}.
-spec run() -> run_result() | error.
run() ->
    run("").

-doc """
Run a test given by either the test id from the last list() command, or
a regex that matches exactly one test. Tests are run with the shortest possible
setup. This call does recompile the test suite and its dependencies. You can
manually recompile code with c(Module). To reset the test state use reset().
""".
-spec run(string() | non_neg_integer()) -> run_result() | error.
run(RegExOrId) ->
    ensure_initialized(),
    case discover(RegExOrId) of
        [] ->
            {0, 0};
        ToRun ->
            Suites = [maps:get(suite, TestMap) || TestMap <- ToRun],
            case shell_buck2_utils:rebuild_modules(Suites) of
                ok ->
                    io:format("Reloading all changed modules... "),
                    case ct_daemon:load_changed() of
                        Loaded when is_list(Loaded) ->
                            case erlang:length(Loaded) of
                                0 ->
                                    do_plain_test_run(ToRun);
                                ChangedCount ->
                                    io:format("reloaded ~tp modules ~tP~n", [ChangedCount, Loaded, 10]),
                                    % There were some changes, so list the tests again, then run but without recompiling changes
                                    % Note that if called with the RegEx instead of ToRun test list like above, do_plain_test_run/1 will list the tests again
                                    do_plain_test_run(RegExOrId)
                            end;
                        node_down ->
                            io:format("Node down!~n"),
                            error
                    end;
                Error ->
                    Error
            end
    end.

-doc """
restarts the test node, enabling a clean test state
""".
-spec reset() -> ok | {error, term()}.
reset() ->
    case is_debug_session() of
        true ->
            io:format(standard_error, "Cannot reset the test node during a debug session!", []);
        false ->
            Type = ct_daemon_node:get_domain_type(),
            NodeName = ct_daemon_node:stop(),
            #test_info{erl_cmd = ErlCmd} = get_provided_test_info(),
            ct_daemon:start(ErlCmd, #{
                type => Type, name => NodeName, cookie => erlang:get_cookie(), options => []
            })
    end.

-doc """
Print all the logs of the currently running test suites
""".
-spec logs() -> ok.
logs() ->
    ensure_initialized(),
    case logs_impl() of
        {ok, Logs} ->
            lists:foreach(fun(LogPath) -> io:format("~ts~n", [LogPath]) end, Logs),
            io:format("~n");
        {error, not_found} ->
            io:format("no logs found~n")
    end.

%% internal
-spec list_impl(RegEx :: string()) -> {ok, string()} | {error, Error} when
    Error :: node_down | {invalid_regex, {string(), non_neg_integer()}}.
list_impl(RegEx) ->
    ensure_initialized(),
    case ct_daemon:list(RegEx) of
        Tests when is_list(Tests) -> {ok, print_tests(Tests)};
        {invalid_regex, _} = Err -> {error, Err};
        node_down -> {error, node_down}
    end.

-spec ensure_initialized() -> ok.
ensure_initialized() ->
    PrintInit = lists:foldl(
        fun(Fun, Acc) -> Fun() orelse Acc end,
        false,
        [
            fun init_utility_apps/0,
            fun init_node/0
        ]
    ),
    case PrintInit of
        true ->
            io:format(">> initialization done << ~n", []);
        false ->
            ok
    end.

-spec init_utility_apps() -> boolean().
init_utility_apps() ->
    UtilityApps = application:get_env(test_cli_lib, utility_applications, []),
    StartedApps = #{App => true || {App, _} <- proplists:get_value(started, application:info())},
    StartResults = [init_utility_app(StartedApps, UtilityApp) || UtilityApp <- UtilityApps],
    lists:any(fun(B) when is_boolean(B) -> B end, StartResults).

-spec init_utility_app(StartedApps :: #{atom() => term()}, UtilityApp :: atom()) -> boolean().
init_utility_app(StartedApps, UtilityApp) ->
    case StartedApps of
        #{UtilityApp := _} ->
            false;
        _ ->
            io:format("starting utility application ~ts...~n", [UtilityApp]),
            {Mod, Fun} = application:get_env(test_cli_lib, application_starter, {application, ensure_all_started}),
            case Mod:Fun(UtilityApp) of
                {ok, _} ->
                    true;
                Error ->
                    abort("could not start utility applications:~n~tp", [Error])
            end
    end.

-spec init_common_app_env(#{binary() => binary()}) -> ok.
init_common_app_env(CommonAppEnv) ->
    case map_size(CommonAppEnv) of
        0 ->
            ok;
        _ ->
            case application:load(common) of
                ok -> ok;
                {error, {already_loaded, common}} -> ok
            end,
            maps:foreach(
                fun(Key, Value) ->
                    KeyAtom = binary_to_atom(Key, utf8),
                    % Only set the env if it's not already set to allow cli overrides
                    case application:get_env(common, KeyAtom) of
                        undefined ->
                            ValueTerm = buck_ct_parser:parse_str(Value),
                            application:set_env(common, KeyAtom, ValueTerm);
                        _ ->
                            ok
                    end
                end,
                CommonAppEnv
            ),
            ok
    end.

-define(TYPE_IS_OK(Type), (Type =:= shortnames orelse Type =:= longnames)).

-spec init_node() -> boolean().
init_node() ->
    case ct_daemon:alive() of
        true ->
            false;
        false ->
            io:format("starting test node...~n", []),
            #test_info{erl_cmd = ErlCmd, common_app_env = CommonAppEnv} = get_provided_test_info(),
            init_common_app_env(CommonAppEnv),
            case application:get_env(test_cli_lib, node_config) of
                undefined ->
                    ct_daemon:start(ErlCmd);
                {ok, {Type, NodeName, Cookie}} when ?TYPE_IS_OK(Type), is_atom(NodeName), is_atom(Cookie) ->
                    ct_daemon:start(
                        ErlCmd,
                        #{
                            name => NodeName,
                            type => Type,
                            cookie => Cookie,
                            options => [{multiply_timetraps, infinity} || is_debug_session()]
                        }
                    )
            end,
            case is_debug_session() of
                true ->
                    spawn(fun watchdog/0);
                false ->
                    ok
            end,
            true
    end.

-spec get_provided_test_info() -> provided_test_info().
get_provided_test_info() ->
    case application:get_env(test_cli_lib, test_info_file, undefined) of
        undefined ->
            abort("test_info_file not provided.");
        TestInfoFile when is_binary(TestInfoFile) ->
            test_info:load_from_file(TestInfoFile)
    end.

-spec abort(Message :: string()) -> no_return().
abort(Message) ->
    abort(Message, []).

-spec abort(Format :: string(), Args :: [term()]) -> no_return().
abort(Format, Args) ->
    io:format(standard_error, "ERROR: " ++ Format ++ "~n", Args),
    io:format(standard_error, "exiting...~n", []),
    erlang:halt(1).

-spec watchdog() -> no_return().
watchdog() ->
    Node = ct_daemon_node:get_node(),
    true = erlang:monitor_node(Node, true),
    receive
        {nodedown, Node} ->
            io:format(
                standard_error,
                "The debugging session ended, terminating the test shell...~n",
                []
            ),
            erlang:halt()
    end.

-spec print_tests([{module(), [{non_neg_integer(), string()}]}]) -> string().
print_tests([]) ->
    "no tests found\n";
print_tests(Tests) ->
    lists:flatten(print_tests_impl(lists:reverse(Tests))).

-spec print_tests_impl([{module(), [{non_neg_integer(), string()}]}]) -> io_lib:chars().
print_tests_impl([]) ->
    "";
print_tests_impl([{Suite, SuiteTests} | Rest]) ->
    SuiteString = io_lib:format("~ts:~n", [Suite]),
    TestsString = [io_lib:format("\t~b - ~ts~n", [Id, Test]) || {Id, Test} <- SuiteTests],
    RestString = print_tests_impl(Rest),
    SuiteString ++ TestsString ++ RestString.

-spec is_debug_session() -> boolean().
is_debug_session() ->
    case application:get_env(test_cli_lib, debugger_mode, false) of
        Value when is_boolean(Value) ->
            Value
    end.

-spec collect_results(#{module() => [string()]}) -> #{string() => ct_daemon_core:run_result()}.
collect_results(PerSuite) ->
    maps:fold(
        fun(Suite, Tests, Acc) ->
            %% check if we need to reset the test node
            ensure_per_suite_encapsulation(Suite),
            io:format("running ~b test(s) for ~ts with output dir ~ts~n", [
                erlang:length(Tests), Suite, ct_daemon:output_dir()
            ]),
            %% run all tests for the current SUITE
            case ct_daemon:run({discovered, [#{suite => Suite, name => Test} || Test <- Tests]}) of
                node_down ->
                    io:format("test node shut down during test execution, aborting~n", []),
                    Acc;
                {error, Reason} ->
                    io:format("Error selecting tests to run: ~p~n", [Reason]),
                    Acc;
                RunResult ->
                    maps:merge(
                        Acc,
                        RunResult
                    )
            end
        end,
        #{},
        PerSuite
    ).

-spec ensure_per_suite_encapsulation(module()) -> ok.
ensure_per_suite_encapsulation(Suite) ->
    case ct_daemon:setup_state() of
        undefined ->
            ok;
        Setup ->
            case lists:reverse(Setup) of
                [Suite | _] ->
                    ok;
                _ ->
                    %% restart node and preserver listing
                    reset(),
                    ok
            end
    end.

-spec discover(string() | non_neg_integer() | atom()) -> [test_info()].
discover(RegExOrId) ->
    StringOrId =
        case is_atom(RegExOrId) of
            true ->
                atom_to_list(RegExOrId);
            false ->
                RegExOrId
        end,
    case ct_daemon:discover(StringOrId) of
        {error, not_listed_yet} ->
            ct_daemon:list(""),
            discover(RegExOrId);
        {error, Reason} ->
            io:format("cannot run tests ~0p: ~0p~n", [RegExOrId, Reason]),
            [];
        node_down ->
            io:format("test node unexpectedly down~n"),
            [];
        [] ->
            io:format("no tests found for ~0p~n", [RegExOrId]),
            [];
        Tests ->
            Tests
    end.

-spec do_plain_test_run(run_spec()) -> run_result().
do_plain_test_run([#{} | _] = ToRun) ->
    PerSuite = maps:groups_from_list(
        fun(#{suite := Suite}) -> Suite end,
        fun(#{name := Name}) -> Name end,
        ToRun
    ),
    Results = collect_results(PerSuite),
    io:format("~n", []),
    Result =
        {Passed, Total} = maps:fold(
            fun(Name, Result, {Passed, Total}) ->
                ct_daemon_printer:print_result(Name, Result),
                case Result of
                    pass_result -> {Passed + 1, Total + 1};
                    _ -> {Passed, Total + 1}
                end
            end,
            {0, 0},
            Results
        ),
    ct_daemon_printer:print_summary(Total, Passed, Total - Passed),
    Result;
do_plain_test_run(RegExOrId) ->
    case discover(RegExOrId) of
        [] -> {0, 0};
        ToRun -> do_plain_test_run(ToRun)
    end.

-spec start_shell() -> ok | {error, already_started}.
start_shell() ->
    shell:start_interactive().

-spec logs_impl() -> {ok, [file:filename_all()]} | {error, not_found}.
logs_impl() ->
    case ct_daemon:priv_dir() of
        undefined ->
            {error, not_found};
        PrivDir ->
            PatternLog = filename:join(PrivDir, "*.log"),
            LogPaths = filelib:wildcard(PatternLog, ".", ?raw_file_access),
            PatternLogJson = filename:join(PrivDir, "*.log.json"),
            LogJsonPaths = filelib:wildcard(PatternLogJson, ".", ?raw_file_access),
            AllLogs = lists:sort(LogPaths ++ LogJsonPaths),
            {ok, AllLogs}
    end.
