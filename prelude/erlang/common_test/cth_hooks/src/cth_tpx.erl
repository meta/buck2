%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(cth_tpx).
-compile(warn_missing_spec_all).

-export([is_running_in_sandcastle/0]).

%% Callbacks
-export([id/1]).
-export([init/2]).

-export([pre_init_per_suite/3]).
-export([post_init_per_suite/4]).
-export([pre_end_per_suite/3]).
-export([post_end_per_suite/4]).

-export([pre_init_per_group/4]).
-export([post_init_per_group/5]).
-export([pre_end_per_group/4]).
-export([post_end_per_group/5]).

-export([pre_init_per_testcase/4]).
-export([post_init_per_testcase/5]).
-export([pre_end_per_testcase/4]).
-export([post_end_per_testcase/5]).

-export([on_tc_fail/4]).
-export([on_tc_skip/4]).

-export([terminate/1]).

%% please dialyzer
-export([ok_group/1, fail_group/1]).

-export_type([
    shared_state/0
]).

-import(common_util, [unicode_characters_to_list/1]).

%% For tests purposes

-include_lib("kernel/include/logger.hrl").
-include("method_ids.hrl").

%% ----------------------- Types --------------------------

%  `SUCCESS`, `FAILURE`, `ASSUMPTION_VIOLATION`, `DISABLED`, `EXCLUDED`, `DRY_RUN`

%% -----------------------------------------------------------------------------
%%            Types
%% -----------------------------------------------------------------------------

-type tree_node() :: cth_tpx_test_tree:tree_node().
-type group_path() :: cth_tpx_test_tree:group_path().
-type outcome() :: cth_tpx_test_tree:outcome().

-record(state, {
    cached_user_process :: pid(),
    suite :: ct_suite(),
    groups :: group_path(),
    start_info :: #{method_id() => start_info()},
    ct_stdout_fingerprint :: ct_stdout:fingerprint(),
    tree_results :: tree_node(),
    previous_group_failed :: boolean() | undefined,
    output :: {file, file:filename_all()} | stdout
}).

-type hook_opts() :: #{
    role := cth_tpx_role:role(),
    ct_stdout_fingerprint := ct_stdout:fingerprint(),
    result_json => file:filename_all()
}.

-type shared_state() :: #state{}.
-type hook_state() :: #{
    id := term(),
    role := cth_tpx_role:role(),
    server := cth_tpx_server:handle()
}.
-type start_info() :: #{
    timestamp := float(),
    stdout_progress_line := ct_stdout:progress_line()
}.

-type ct_suite() :: module().
-type ct_groupname() :: ct_suite:ct_groupname().
-type ct_testname() :: ct_suite:ct_testname().
-type ct_config() :: ct_suite:ct_config().
-type ct_config_or_skip_or_fail() ::
    ct_config() | {skip, term()} | {fail, term()}.
-type ct_config_or_skip_or_fail_or_term() ::
    ct_config_or_skip_or_fail() | ok | term().

%% -----------------------------------------------------------------------------

-spec second_timestamp() -> float().
second_timestamp() ->
    os:system_time(millisecond) / 1000.

%% -----------------------------------------------------------------------------
%%    Registering and collecting results.
%% -----------------------------------------------------------------------------

% General workflow:
% ct will call methods pre_ post_ method before each method init, case, end methods from
% the test_suite.
% Based on the state in each of these, we create a result that will be passed to the method
% add_result/4.
% This one will register the results into a tree, using the method cth_tpx_test_tree:register_result/4.
% Once the whole run is finished, the method terminate/1 is called.
% This one will, for each requested_test creates and output a method_result, using the
% previously constructed tree_result.

%%%%%%%%%%%%%%%%%% This part is similar to the one in cth_tespilot (execpt for some minor modifications
%% in representing init / main/ end testcases as {Phase, Name})

%% -----------------------------------------------------------------------------
%% Format Functions
%% -----------------------------------------------------------------------------

-spec fmt_skip(Reason) -> unicode:chardata() when
    Reason :: term().
fmt_skip(Reason) ->
    fmt_stack(Reason, "SKIPPED").

-spec fmt_fail(Reason) -> unicode:chardata() when
    Reason :: term().
fmt_fail(Reason) ->
    fmt_stack(Reason, "FAILED").

-spec fmt_init_or_end(Suite, Callback, Reason, Label) -> unicode:chardata() when
    Suite :: ct_suite(),
    Callback :: init_per_suite | end_per_suite | init_per_group | end_per_group | init_per_testcase | end_per_testcase,
    Reason :: term(),
    Label :: unicode:chardata().
fmt_init_or_end(Suite, Callback, {failed, {Suite, Callback, ActualReason}}, Label) ->
    fmt_init_or_end(Suite, Callback, ActualReason, Label);
fmt_init_or_end(_Suite, Callback, Reason, Label) ->
    fmt_stack(Reason, io_lib:format("~ts ~ts", [Callback, Label])).

-spec fmt_stack(Reason, Label) -> unicode:chardata() when
    Reason :: term(),
    Label :: unicode:chardata().
fmt_stack({'EXIT', {Reason, ST}}, Label) ->
    fmt_stack({Reason, ST}, Label);
fmt_stack(Reason, _Label) ->
    Output = ct_error_printer:format_error(Reason, true),
    io_lib:format("~ts", [Output]).

-doc """
Whether a crash asks to be reported to tpx as an infra failure rather than a test
failure.

A test or callback opts in by crashing with `error(test_infra_error)` or
`error({test_infra_error, Reason})`, or via a configured `infra_failure_classifier`
module. tpx retries infra failures transparently and records them outside the
failure band, so they do not break a diff.

Only the wrapper shapes CT itself adds are unwrapped. The marker is deliberately not
searched for inside arbitrary terms, so an ordinary failure that happens to carry the
atom somewhere in its payload stays an ordinary failure.
""".
-spec is_infra_error(Reason) -> boolean() when Reason :: term().
is_infra_error(test_infra_error) ->
    true;
is_infra_error({test_infra_error, _Reason}) ->
    true;
is_infra_error({failed, {_Suite, _Callback, Reason}}) ->
    is_infra_error(Reason);
is_infra_error({'EXIT', Reason}) ->
    is_infra_error(Reason);
is_infra_error({'EXIT', _Pid, Reason}) ->
    is_infra_error(Reason);
is_infra_error({Reason, Stacktrace}) when is_list(Stacktrace) ->
    is_infra_error(Reason);
is_infra_error({Tag, Reason}) when Tag =:= skip; Tag =:= fail; Tag =:= error ->
    is_infra_error(Reason);
is_infra_error(Reason) ->
    classify_infra_failure(Reason).

-spec classify_infra_failure(term()) -> boolean().
classify_infra_failure(Reason) ->
    case common_util:get_env(infra_failure_classifier) of
        {ok, Classifier} when is_atom(Classifier) ->
            try
                Classifier:is_infra_failure(Reason) =:= true
            catch
                Class:Error:Stack ->
                    ?LOG_WARNING(
                        "infra_failure_classifier ~tp:is_infra_failure/1 failed: ~ts",
                        [Classifier, erl_error:format_exception(Class, Error, Stack)]
                    ),
                    false
            end;
        _ ->
            false
    end.

-doc """
The outcome to record for a failing method: `infra_failure` if it opted in via
`test_infra_error` or is classified by the configured `infra_failure_classifier`,
otherwise the usual `failed`.
""".
-spec fail_outcome(Reason) -> failed | infra_failure when Reason :: term().
fail_outcome(Reason) ->
    case is_infra_error(Reason) of
        true -> infra_failure;
        false -> failed
    end.

%% -----------------------------------------------------------------------------
%% CT hooks functions
%% -----------------------------------------------------------------------------

-doc """
Return a unique id for this CTH.
""".
-spec id(hook_opts()) -> {?MODULE, cth_tpx_role:role()}.
id(#{role := Role}) ->
    {?MODULE, Role}.

-doc """
Always called before any other callback function. Use this to initiate
any common state.
""".
-spec init(_Id :: term(), Opts :: hook_opts()) -> {ok, hook_state(), integer()}.
init(Id, Opts = #{role := Role}) ->
    ServerName = '$cth_tpx$server$',
    case Role of
        top ->
            Output =
                case maps:get(result_json, Opts, undefined) of
                    undefined -> stdout;
                    FN -> {file, FN}
                end,
            #{ct_stdout_fingerprint := Fingerprint} = Opts,
            init_role_top(Id, ServerName, Fingerprint, Output);
        bot ->
            init_role_bot(Id, ServerName)
    end.

-spec init_role_top(Id, ServerName, Fingerprint, Output) ->
    {ok, hook_state(), integer()}
when
    Id :: term(),
    ServerName :: atom(),
    Fingerprint :: ct_stdout:fingerprint(),
    Output :: stdout | {file, file:filename_all()}.
init_role_top(Id, ServerName, Fingerprint, Output) ->
    CachedUserProcess =
        case whereis(user) of
            UserPid when is_pid(UserPid) -> UserPid
        end,

    SharedState = #state{
        cached_user_process = CachedUserProcess,
        output = Output,
        ct_stdout_fingerprint = Fingerprint,
        start_info = #{},
        groups = [],
        tree_results = cth_tpx_test_tree:new_node(unknown_suite)
    },
    Handle = cth_tpx_server:start_link(SharedState),
    % Register so that init_role_bot can find it
    register(ServerName, Handle),
    HookState = #{
        id => Id,
        role => top,
        server => Handle
    },
    {ok, HookState, cth_tpx_role:role_priority(top)}.

-spec init_role_bot(Id, ServerName) -> {ok, hook_state(), integer()} when
    Id :: term(), ServerName :: atom().
init_role_bot(Id, ServerName) ->
    % Put there by init_role_top
    case whereis(ServerName) of
        Handle when is_pid(Handle) ->
            unregister(ServerName),
            HookState = #{
                id => Id,
                role => bot,
                server => Handle
            },
            {ok, HookState, cth_tpx_role:role_priority(bot)}
    end.

-doc """
Called before init_per_suite is called.
""".
-spec pre_init_per_suite(ct_suite(), ct_config(), hook_state()) -> {ct_config_or_skip_or_fail(), hook_state()}.
pre_init_per_suite(Suite, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        State1 = collect_start_info(State, ?INIT_PER_SUITE),
        {Config, State1#state{
            suite = Suite,
            groups = [],
            tree_results = cth_tpx_test_tree:new_node(Suite),
            previous_group_failed = false
        }}
    end).

-spec post_init_per_suite(ct_suite(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_init_per_suite(Suite, _Config, {skip, {failed, _} = Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_init_or_end(Suite, init_per_suite, Reason, ~"FAILED"),
        {Error, add_result(?INIT_PER_SUITE, fail_outcome(Reason), Desc, State)}
    end);
post_init_per_suite(Suite, _Config, {skip, Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_init_or_end(Suite, init_per_suite, Reason, ~"SKIPPED"),
        {Error, add_result(?INIT_PER_SUITE, skipped, Desc, State)}
    end);
post_init_per_suite(Suite, _Config, {fail, Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_init_or_end(Suite, init_per_suite, Reason, ~"FAILED"),
        {Error, add_result(?INIT_PER_SUITE, fail_outcome(Reason), Desc, State)}
    end);
post_init_per_suite(Suite, _Config, Error, HookState) when not is_list(Error) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        Desc = fmt_init_or_end(Suite, init_per_suite, Error, ~"FAILED"),
        {Error, add_result(?INIT_PER_SUITE, fail_outcome(Error), Desc, State)}
    end);
post_init_per_suite(_Suite, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State) ->
        {Return, add_result(?INIT_PER_SUITE, passed, ~"", State)}
    end).

-spec pre_end_per_suite(ct_suite(), ct_config(), hook_state()) -> {ct_config(), hook_state()}.
pre_end_per_suite(_Suite, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        {Config, collect_start_info(State, ?END_PER_SUITE)}
    end).

-spec post_end_per_suite(ct_suite(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_end_per_suite(
    Suite,
    _Config,
    {skip, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        Desc = fmt_init_or_end(Suite, end_per_suite, Reason, ~"SKIPPED"),
        State1 = add_result(?END_PER_SUITE, skipped, Desc, State0),
        {Error, clear_suite(State1)}
    end);
%% Workaround OTP bug (https://github.com/erlang/otp/issues/11647):
%% `test_server:process_return_val1` does not treat a `{fail, Reason}` return from
%% `end_per_suite` as a failure (and an earlier CTH's `post_end_per_suite` returning
%% `{fail, Reason}` does not update `Config`), so `Config` has no `{tc_status, ...}`
%% entry. Match `{fail, Reason}` in `Return` explicitly.
post_end_per_suite(
    Suite,
    _Config,
    {fail, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        Desc = fmt_init_or_end(Suite, end_per_suite, Reason, ~"FAILED"),
        State1 = add_result(?END_PER_SUITE, fail_outcome(Reason), Desc, State0),
        {Error, clear_suite(State1)}
    end);
post_end_per_suite(Suite, Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0) ->
        State1 =
            case lists:keyfind(tc_status, 1, Config) of
                {tc_status, Status} when Status =/= ok ->
                    Reason =
                        case Status of
                            {failed, Reason0} -> Reason0;
                            _ -> Status
                        end,
                    Desc = fmt_init_or_end(Suite, end_per_suite, Reason, ~"FAILED"),
                    add_result(?END_PER_SUITE, fail_outcome(Reason), Desc, State0);
                _ ->
                    add_result(?END_PER_SUITE, passed, ~"", State0)
            end,
        {Return, clear_suite(State1)}
    end).

-spec clear_suite(#state{}) -> #state{}.
clear_suite(State) ->
    State#state{
        suite = undefined,
        groups = [],
        start_info = #{}
    }.

-spec pre_init_per_group(ct_suite(), ct_groupname(), ct_config(), hook_state()) -> {ct_config(), hook_state()}.
pre_init_per_group(_SuiteName, _Group, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun
        (State = #state{groups = [_ | Groups], previous_group_failed = true}) ->
            State1 = collect_start_info(State, ?INIT_PER_GROUP),
            {Config, State1#state{groups = Groups, previous_group_failed = false}};
        (#state{} = State) ->
            {Config, collect_start_info(State, ?INIT_PER_GROUP)}
    end).

-spec post_init_per_group(ct_suite(), ct_groupname(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_init_per_group(
    Suite,
    Group,
    _Config,
    {skip, {failed, _} = Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_init_or_end(Suite, init_per_group, Reason, ~"FAILED"),
        State2 = add_result(?INIT_PER_GROUP, fail_outcome(Reason), Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(
    Suite,
    Group,
    _Config,
    {skip, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_init_or_end(Suite, init_per_group, Reason, ~"SKIPPED"),
        State2 = add_result(?INIT_PER_GROUP, skipped, Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(
    Suite,
    Group,
    _Config,
    {fail, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_init_or_end(Suite, init_per_group, Reason, ~"FAILED"),
        State2 = add_result(?INIT_PER_GROUP, fail_outcome(Reason), Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(Suite, Group, _Config, Error, HookState) when not is_list(Error) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        Desc = fmt_init_or_end(Suite, init_per_group, Error, ~"FAILED"),
        State2 = add_result(?INIT_PER_GROUP, fail_outcome(Error), Desc, State1),
        {Error, fail_group(State2)}
    end);
post_init_per_group(_Suite, Group, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0 = #state{groups = Groups}) ->
        State1 = State0#state{groups = [Group | Groups]},
        State2 = add_result(?INIT_PER_GROUP, passed, ~"", State1),
        {Return, ok_group(State2)}
    end).

-spec ok_group(#state{}) -> #state{}.
ok_group(State) ->
    State#state{previous_group_failed = false}.

-spec fail_group(#state{}) -> #state{}.
fail_group(State) ->
    State#state{previous_group_failed = true}.

-spec pre_end_per_group(ct_suite(), ct_groupname(), ct_config(), hook_state()) -> {ct_config(), hook_state()}.
pre_end_per_group(_SuiteName, _Group, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        {Config, collect_start_info(State, ?END_PER_GROUP)}
    end).

-spec post_end_per_group(ct_suite(), ct_groupname(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_end_per_group(
    Suite,
    _Group,
    _Config,
    {skip, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        Desc = fmt_init_or_end(Suite, end_per_group, Reason, ~"SKIPPED"),
        State1 = add_result(?END_PER_GROUP, skipped, Desc, State0),
        {Error, State1#state{groups = tl(Groups)}}
    end);
%% Workaround OTP bug (https://github.com/erlang/otp/issues/11647):
%% `test_server:process_return_val1` does not treat a `{fail, Reason}` return from
%% `end_per_group` as a failure (and an earlier CTH's `post_end_per_group` returning
%% `{fail, Reason}` does not update `Config`), so `Config` has no `{tc_status, ...}`
%% entry. Match `{fail, Reason}` in `Return` explicitly.
post_end_per_group(
    Suite,
    _Group,
    _Config,
    {fail, Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0 = #state{groups = Groups}) ->
        Desc = fmt_init_or_end(Suite, end_per_group, Reason, ~"FAILED"),
        State1 = add_result(?END_PER_GROUP, fail_outcome(Reason), Desc, State0),
        {Error, State1#state{groups = tl(Groups)}}
    end);
post_end_per_group(Suite, _Group, Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0 = #state{groups = Groups}) ->
        State1 =
            case lists:keyfind(tc_status, 1, Config) of
                {tc_status, Status} when Status =/= ok ->
                    Reason =
                        case Status of
                            {failed, Reason0} -> Reason0;
                            _ -> Status
                        end,
                    Desc = fmt_init_or_end(Suite, end_per_group, Reason, ~"FAILED"),
                    add_result(?END_PER_GROUP, fail_outcome(Reason), Desc, State0);
                _ ->
                    add_result(?END_PER_GROUP, passed, ~"", State0)
            end,
        {Return, State1#state{groups = tl(Groups)}}
    end).

-spec pre_init_per_testcase(ct_suite(), ct_testname(), ct_config(), hook_state()) -> {ct_config(), hook_state()}.
pre_init_per_testcase(_Suite, TestCase, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        %% store name and start time for current test case
        %% We capture time twice:
        %%  1) For the init_per_testcase.
        %%  2) For the whole testcase = init + actual_testcase + end
        %% The reason behind is that capturing the timing for the actual_testcase
        %% is not straightforward, as there is no pre/post method for it.
        State1 = collect_start_info(State, {TestCase, ?INIT_PER_TESTCASE}),
        State2 = collect_start_info(State1, {TestCase, ?MAIN_TESTCASE}),
        {Config, State2}
    end).

-spec post_init_per_testcase(ct_suite(), ct_testname(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_init_per_testcase(
    Suite,
    TestCase,
    _Config,
    {skip, {failed, _} = Reason} = Error,
    HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        %% ct skip because of failed init is reported as error
        Desc = fmt_init_or_end(Suite, init_per_testcase, Reason, ~"FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, fail_outcome(Reason), Desc, State)}
    end);
post_init_per_testcase(
    Suite, TestCase, _Config, {skip, Reason} = Error, HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        %% other skips (user skip) are reported as skips
        Desc = fmt_init_or_end(Suite, init_per_testcase, Reason, ~"SKIPPED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, skipped, Desc, State)}
    end);
post_init_per_testcase(
    Suite, TestCase, _Config, {fail, Reason} = Error, HookState
) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        %% fails are reported as errors
        Desc = fmt_init_or_end(Suite, init_per_testcase, Reason, ~"FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, fail_outcome(Reason), Desc, State)}
    end);
post_init_per_testcase(Suite, TestCase, _Config, {error, Reason} = Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        %% terms are reported as errors except ok (missing in CT doc)
        Desc = fmt_init_or_end(Suite, init_per_testcase, Reason, ~"FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, fail_outcome(Reason), Desc, State)}
    end);
post_init_per_testcase(Suite, TestCase, _Config, Error, HookState) when
    not is_list(Error) andalso ok =/= Error
->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State) ->
        %% terms are reported as errors except ok (missing in CT doc)
        Desc = fmt_init_or_end(Suite, init_per_testcase, Error, ~"FAILED"),
        {Error, add_result({TestCase, ?INIT_PER_TESTCASE}, fail_outcome(Error), Desc, State)}
    end);
post_init_per_testcase(_Suite, TestCase, _Config, Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State) ->
        %% everything else is ok
        State1 = add_result({TestCase, ?INIT_PER_TESTCASE}, passed, ~"", State),
        {Return, State1}
    end).

%% add test result to state
-spec add_result(method_id(), outcome(), unicode:chardata(), shared_state()) -> shared_state().
add_result(
    Method,
    Outcome,
    Desc,
    State = #state{
        groups = Groups,
        start_info = StartInfo0,
        tree_results = TreeResults
    }
) ->
    QualifiedName = method_name(Method, Groups),
    TS = second_timestamp(),
    Result0 = #{
        name => QualifiedName,
        outcome => Outcome,
        details => unicode_characters_to_list(Desc)
    },
    Result =
        case StartInfo0 of
            #{Method := #{timestamp := StartTime, stdout_progress_line := StartStdOutLine}} ->
                EndStdOutLine = ct_stdout:emit_progress(
                    State#state.cached_user_process,
                    State#state.ct_stdout_fingerprint,
                    method_progress_label(Method),
                    finished
                ),

                Result0#{
                    start_time => StartTime,
                    end_time => TS,
                    start_progress_marker => StartStdOutLine,
                    end_progress_marker => EndStdOutLine
                };
            _ ->
                %% If no test data (skipped test cases/groups/suite)
                %% then started time doesn't exist.
                Result0
        end,
    ST1 = maps:remove(Method, StartInfo0),
    NewTreeResults = cth_tpx_test_tree:register_result(TreeResults, Result, Groups, Method),
    State#state{start_info = ST1, tree_results = NewTreeResults}.

-spec method_name(method_id(), group_path()) -> string().
method_name(Method, Groups) ->
    MethodName =
        case Method of
            {TestCase, Phase} ->
                unicode_characters_to_list(io_lib:format("~ts.~ts", [atom_to_list(TestCase), atom_to_list(Phase)]));
            MethodName0 ->
                atom_to_list(MethodName0)
        end,
    common_util:qualified_name(Groups, MethodName).

-spec pre_end_per_testcase(ct_suite(), ct_testname(), ct_config(), hook_state()) -> {ct_config(), hook_state()}.
pre_end_per_testcase(_Suite, TC, Config, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Config, fun(State) ->
        {Config, collect_start_info(State, {TC, ?END_PER_TESTCASE})}
    end).

-spec post_end_per_testcase(ct_suite(), ct_testname(), ct_config(), ct_config_or_skip_or_fail_or_term(), hook_state()) ->
    {ct_config_or_skip_or_fail_or_term(), hook_state()}.
post_end_per_testcase(_Suite, TC, _Config, ok = Return, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Return, fun(State0) ->
        State1 = add_result({TC, ?END_PER_TESTCASE}, passed, ~"", State0),
        {ok, add_result({TC, ?MAIN_TESTCASE}, passed, ~"", State1)}
    end);
post_end_per_testcase(Suite, TC, Config, Error, HookState) ->
    on_shared_state(HookState, ?FUNCTION_NAME, Error, fun(State0) ->
        NextState =
            case lists:keyfind(tc_status, 1, Config) of
                {tc_status, ok} ->
                    %% Test case passed, but we still ended in an error
                    %% same description as ct output
                    %% first report testcase itself
                    State1 = add_result({TC, ?MAIN_TESTCASE}, passed, ~"", State0),
                    %% now report end per failure
                    Reason =
                        case Error of
                            {skip, Reason0} -> Reason0;
                            {fail, Reason0} -> Reason0;
                            _ -> Error
                        end,
                    Desc = fmt_init_or_end(Suite, end_per_testcase, Reason, ~"FAILED"),
                    add_result({TC, ?END_PER_TESTCASE}, fail_outcome(Reason), Desc, State1);
                _ ->
                    %% Test case failed, in which case on_tc_fail already reports it
                    add_result({TC, ?END_PER_TESTCASE}, passed, ~"", State0)
            end,
        {Error, NextState}
    end).

-spec on_tc_fail(Suite, Callback, Reason, HookState) -> hook_state() when
    Suite :: ct_suite(),
    Callback ::
        init_per_suite
        | end_per_suite
        | ct_testname()
        | {ct_testname(), ct_groupname()}
        | {init_per_group | end_per_group, ct_groupname()},
    Reason :: term(),
    HookState :: hook_state().
on_tc_fail(_SuiteName, init_per_suite, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, end_per_suite, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {init_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {end_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_fail(_SuiteName, {TC, _Group}, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        Desc = fmt_fail(Reason),
        add_result({TC, ?MAIN_TESTCASE}, fail_outcome(Reason), Desc, State)
    end);
on_tc_fail(_SuiteName, TC, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        Desc = fmt_fail(Reason),
        add_result({TC, ?MAIN_TESTCASE}, fail_outcome(Reason), Desc, State)
    end).

-spec on_tc_skip(Suite, Callback, Reason, HookState) -> hook_state() when
    Suite :: ct_suite(),
    Callback ::
        init_per_suite
        | end_per_suite
        | ct_testname()
        | {ct_testname(), ct_groupname()}
        | {init_per_group | end_per_group, ct_groupname()},
    Reason :: {tc_auto_skip, term()} | {tc_user_skip, term()},
    HookState :: hook_state().
on_tc_skip(_SuiteName, init_per_suite, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, end_per_suite, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {init_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {end_per_group, _GroupName}, _, HookState) ->
    HookState;
on_tc_skip(_SuiteName, {TC, _Group}, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        handle_on_tc_skip(TC, Reason, State)
    end);
on_tc_skip(_SuiteName, TC, Reason, HookState) ->
    modify_shared_state(HookState, ?FUNCTION_NAME, fun(State) ->
        handle_on_tc_skip(TC, Reason, State)
    end).

-spec handle_on_tc_skip(TC, Reason, State) -> State when
    TC :: ct_testname(),
    Reason :: {tc_auto_skip, term()} | {tc_user_skip, term()},
    State :: #state{}.
handle_on_tc_skip(TC, {tc_auto_skip, Reason}, State = #state{suite = Suite}) ->
    Desc = fmt_fail(Reason),
    NewState = add_result({TC, ?MAIN_TESTCASE}, fail_outcome(Reason), Desc, State),
    NewState#state{suite = Suite};
handle_on_tc_skip(TC, {tc_user_skip, Reason}, State = #state{suite = Suite}) ->
    Desc = fmt_skip(Reason),
    NewState = add_result({TC, ?MAIN_TESTCASE}, skipped, Desc, State),
    NewState#state{suite = Suite}.

-doc """
Called when the scope of the CTH is done
""".
-spec terminate(hook_state()) -> ok | {error, _Reason}.
terminate(#{role := top, server := Handle}) ->
    #state{output = Output, tree_results = TreeResults} = cth_tpx_server:get(Handle),
    write_output(Output, term_to_binary(TreeResults));
terminate(#{role := bot}) ->
    ok.

-spec write_output({file, file:filename_all()} | stdout, binary()) -> ok.
write_output({file, FN}, JSON) ->
    io:format("Writing result file ~tp~n", [FN]),
    ok = filelib:ensure_dir(FN),
    ok = file:write_file(FN, JSON, [raw, binary]);
write_output(stdout, JSON) ->
    io:format(user, "~tp", [JSON]).

-spec collect_start_info(State, MethodId) -> shared_state() when
    State :: shared_state(),
    MethodId :: method_id().
collect_start_info(#state{start_info = ST0} = State, MethodId) ->
    ProgressLine = ct_stdout:emit_progress(
        State#state.cached_user_process,
        State#state.ct_stdout_fingerprint,
        method_progress_label(MethodId),
        started
    ),

    StartInfo = #{
        timestamp => second_timestamp(),
        stdout_progress_line => ProgressLine
    },
    State#state{start_info = ST0#{MethodId => StartInfo}}.

-spec method_progress_label(Method) -> unicode:chardata() when
    Method :: method_id().
method_progress_label(Method) ->
    io_lib:format("~tw", [Method]).

-spec on_shared_state(hook_state(), Caller, Default, Action) -> {A, hook_state()} when
    Caller :: cth_tpx_role:responsibility(),
    Default :: A,
    Action :: fun((shared_state()) -> {A, shared_state()}).
on_shared_state(HookState = #{role := Role, server := Handle}, Caller, Default, Action) ->
    case cth_tpx_role:is_responsible(Role, Caller) of
        true ->
            A = cth_tpx_server:modify(Handle, Action),
            {A, HookState};
        false ->
            {Default, HookState}
    end.

-spec modify_shared_state(hook_state(), Caller, Action) -> hook_state() when
    Caller :: cth_tpx_role:responsibility(),
    Action :: fun((shared_state()) -> shared_state()).
modify_shared_state(HookState, Caller, Action) ->
    {ok, NewHookState} = on_shared_state(HookState, Caller, _Default = ok, fun(State) ->
        {ok, Action(State)}
    end),
    NewHookState.

-spec is_running_in_sandcastle() -> boolean().
is_running_in_sandcastle() ->
    case os:getenv("SANDCASTLE_DIFF_ID") of
        [$D | _] ->
            true;
        _ ->
            case os:getenv("SANDCASTLE") of
                false -> false;
                _ -> true
            end
    end.
