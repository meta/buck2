%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(ct_executor).
-moduledoc """
Simple trampoline for ct_run.
Notably allows us to call post/pre method on the node if needed, e.g for coverage.
""".
-compile(warn_missing_spec_all).

-export([run/1]).

% For testing
-export([ct_run_exit_code/1, preload_app_file_atoms/1, split_args/1]).

-include_lib("kernel/include/logger.hrl").
-include_lib("common/include/buck_ct_records.hrl").

%% `ct_run_arg()` represents an option accepted by ct:run_test/1, such as
%% `multiply_timetraps` or `ct_hooks`.
%% For all the options, see https://www.erlang.org/doc/man/ct#run_test-1
-type ct_run_arg() ::
    {dir, dynamic()}
    | {suite, dynamic()}
    | {group, dynamic()}
    | {testcase, dynamic()}
    | {spec, dynamic()}
    | {join_specs, dynamic()}
    | {label, dynamic()}
    | {config, dynamic()}
    | {userconfig, dynamic()}
    | {allow_user_terms, dynamic()}
    | {logdir, dynamic()}
    | {silent_connections, dynamic()}
    | {stylesheet, dynamic()}
    | {cover, dynamic()}
    | {cover_stop, dynamic()}
    | {step, dynamic()}
    | {event_handler, dynamic()}
    | {include, dynamic()}
    | {auto_compile, dynamic()}
    | {abort_if_missing_suites, dynamic()}
    | {create_priv_dir, dynamic()}
    | {multiply_timetraps, dynamic()}
    | {scale_timetraps, dynamic()}
    | {repeat, dynamic()}
    | {duration, dynamic()}
    | {until, dynamic()}
    | {force_stop, dynamic()}
    | {decrypt, dynamic()}
    | {refresh_logs, dynamic()}
    | {logopts, dynamic()}
    | {verbosity, dynamic()}
    | {basic_html, dynamic()}
    | {esc_chars, dynamic()}
    | {keep_logs, dynamic()}
    | {ct_hooks, dynamic()}
    | {ct_hooks_order, dynamic()}
    | {enable_builtin_hooks, dynamic()}
    | {release_shell, dynamic()}.

-type ct_exec_arg() ::
    {output_dir, file:filename()}
    | {server_port, inet:port_number()}
    | {suite, module()}
    | {providers, [{Name :: atom(), Args :: term()}]}
    | {common_app_env, #{binary() => binary()}}.

-type ct_run_result() ::
    {
        Ok :: integer(),
        Failed :: integer(),
        {UserSkipped :: integer(), AutoSkipped :: integer()}
    }
    | (TestRunnerPid :: pid())
    | {error, Reason :: term()}.

-define(raw_file_access, prim_file).

-spec run([string()]) -> no_return().
run(Args) when is_list(Args) ->
    {ok, CWDDir} = prim_file:get_cwd(),
    os:putenv("HOME", CWDDir),
    ExitCode =
        try
            {CtExecutorArgs, CtRunArgs} = parse_arguments(Args),
            debug_print("~tp", [#{ct_exec_args => CtExecutorArgs, ct_run_args => CtRunArgs}]),
            [OutputDir | _] = [OutputDir || {output_dir, OutputDir} <- CtExecutorArgs],
            ok = test_logger:set_up_logger(OutputDir, ?MODULE, no_capture_stdout),
            % Until this point the logger is not set up so we cannot log.
            % Therefore we used io:format to forward information to the
            % process calling it (ct_runner).

            %% log arguments into ct_executor.log
            ?LOG_INFO("raw args: ~tp", [Args]),
            ?LOG_INFO("executor args: ~tp", [CtExecutorArgs]),
            ?LOG_INFO("CtRunArgs: ~tp", [CtRunArgs]),

            try
                %% setup watchdog
                [ServerPort | _] = [ServerPort || {server_port, ServerPort} <- CtExecutorArgs],
                ct_executor_watchdog:start_link_client(ServerPort),

                ok = init_common_app_env(proplists:get_value(common_app_env, CtExecutorArgs, #{})),
                ok = preload_application_atoms(),
                [Suite | _] = [Suite || {suite, Suite} <- CtExecutorArgs],

                {ok, RawTarget} = common_util:get_env(raw_target),

                ProviderInitState = #init_provider_state{output_dir = OutputDir, suite = Suite, raw_target = RawTarget},
                Providers0 = [
                    buck_ct_provider:do_init(Provider, ProviderInitState)
                 || {providers, Providers} <- CtExecutorArgs,
                    Provider <- Providers
                ],

                %% get longer stack traces
                erlang:system_flag(backtrace_depth, 20),
                ?LOG_DEBUG("ct_run called with arguments ~tp ~n", [CtRunArgs]),
                Providers1 = [buck_ct_provider:do_pre_running(Provider) || Provider <- Providers0],

                %% set global timeout
                Result = ct:run_test(CtRunArgs),
                ?LOG_DEBUG("ct_run finished with result ~tp ~n", [Result]),
                Providers2 = [buck_ct_provider:do_post_running(Provider) || Provider <- Providers1],
                [buck_ct_provider:do_terminate(Provider) || Provider <- Providers2],
                ct_run_exit_code(Result)
            catch
                Class:Reason:Stack ->
                    ?LOG_ERROR("ct executor failed due to ~ts\n", [
                        erl_error:format_exception(Class, Reason, Stack)
                    ]),
                    2
            after
                test_logger:flush()
            end
        catch
            % Catch an exception that happens before logging is set up.
            % Will forward the exception to the process that opened the port (ct_runner).
            Class1:Reason1:Stack1 ->
                io:format("~ts\n", [erl_error:format_exception(Class1, Reason1, Stack1)]),
                1
        end,
    erlang:halt(ExitCode).

-spec ct_run_exit_code(ct_run_result()) -> 0 | 2.
ct_run_exit_code({error, Reason}) ->
    ?LOG_ERROR("ct_run failed with reason ~tp", [Reason]),
    2;
ct_run_exit_code(TestRunnerPid) when is_pid(TestRunnerPid) ->
    ?LOG_ERROR("ct_run returned a test runner pid, but release_shell is unsupported: ~tp", [TestRunnerPid]),
    2;
ct_run_exit_code(_Result) ->
    0.

-spec parse_arguments([string()]) -> {[ct_exec_arg()], [ct_run_arg()]}.
parse_arguments(Args) ->
    % The logger is not set up yet.
    % This will be sent to the program executing it (ct_runner),
    % that will log it in its own log.
    debug_print("CT executor called with ~tp~n", [Args]),
    ParsedArgs = [buck_ct_parser:parse_str(Elem) || Elem <:- Args],
    debug_print("Parsed arguments ~tp~n", [ParsedArgs]),
    % We split the arguments between those that go to ct_run and those that are for
    % ct_executor
    % the args passed to ct are to be found after the --ct-args
    split_args(ParsedArgs).

-doc """
Splits the argument before those that happens
before ct_args (the executor args) and those after
(the args for ct_run). ct_args will always be
present in the list
""".
-spec split_args([term()]) -> {[ct_exec_arg()], [ct_run_arg()]}.
split_args(Args) ->
    {CtExecutorArgs, [ct_args | CtRunArgs]} = lists:splitwith(fun(Arg) -> Arg =/= ct_args end, Args),
    {parse_ct_exec_args(CtExecutorArgs), parse_ct_run_args(CtRunArgs)}.

-spec parse_ct_run_args([dynamic()]) -> [ct_run_arg()].
parse_ct_run_args([]) ->
    [];
parse_ct_run_args([{Key, _Value} = Arg | Args]) when is_atom(Key) ->
    [Arg | parse_ct_run_args(Args)].

-spec parse_ct_exec_args([dynamic()]) -> [ct_exec_arg()].
parse_ct_exec_args([]) ->
    [];
parse_ct_exec_args([{Key, _Value} = Arg | Args]) when
    Key =:= output_dir; Key =:= server_port; Key =:= suite; Key =:= providers; Key =:= common_app_env
->
    [Arg | parse_ct_exec_args(Args)].

-spec init_common_app_env(#{binary() => binary()}) -> ok.
init_common_app_env(CommonAppEnv) ->
    ok = load_common_app(),
    maps:foreach(fun init_common_app_env_var/2, CommonAppEnv).

-spec load_common_app() -> ok.
load_common_app() ->
    case application:load(common) of
        ok -> ok;
        {error, {already_loaded, common}} -> ok;
        Error -> error({load_common_app, Error})
    end.

-spec init_common_app_env_var(binary(), binary()) -> ok.
init_common_app_env_var(Key, Value) ->
    KeyAtom = binary_to_atom(Key, utf8),
    case common_util:get_env(KeyAtom) of
        undefined ->
            ValueTerm = buck_ct_parser:parse_str(Value),
            common_util:set_env(KeyAtom, ValueTerm);
        _ ->
            ok
    end.

-spec preload_application_atoms() -> ok.
preload_application_atoms() ->
    PotentialDotApp = [
        filename:join(Dep, filename:basename(filename:dirname(Dep)) ++ ".app")
     || Dep <- code:get_path()
    ],
    lists:foreach(fun preload_app_file_atoms/1, PotentialDotApp),
    ok.

-doc """
Loads atoms referenced by an application resource file into the atom table.
""".
-spec preload_app_file_atoms(file:filename_all()) -> ok.
preload_app_file_atoms(DotApp) ->
    case filelib:is_regular(DotApp, ?raw_file_access) of
        true ->
            case file:read_file(DotApp, [raw]) of
                {ok, DotAppContents} ->
                    % Some tests call *_to_existing_atom for atoms that only appear in dependency .app files.
                    % Tokenizing preserves that side effect without constructing the full terms.
                    case unicode:characters_to_list(DotAppContents) of
                        DotAppString when is_list(DotAppString) ->
                            _ = erl_scan:string(DotAppString),
                            ok;
                        _ ->
                            ok
                    end;
                {error, _} ->
                    ok
            end;
        false ->
            ok
    end.

-spec debug_print(string(), [term()]) -> ok.
debug_print(Fmt, Args) ->
    case os:getenv("ERLANG_BUCK_DEBUG_PRINT") of
        false -> io:format(Fmt, Args);
        "disabled" -> ok;
        _ -> io:format(Fmt, Args)
    end.
