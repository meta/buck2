%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(ct_daemon_hooks).
-moduledoc """
Implementation of hooks functionality. We mimic the behaviour of
common test hooks so that they can run in test shell
""".
-compile(warn_missing_spec_all).

-behaviour(gen_server).

%% API
-export([
    start_monitor/0,
    set_state/2,
    get_state/1,
    wrap/3,
    format_ct_error/3,
    get_hooks/0
]).

%% gen_server callbacks
-export([
    init/1,
    handle_call/3,
    handle_cast/2
]).

-export_type([part/0]).

-type id() :: term().
-type opts() :: term().
-type hook_state() :: term().
-type group_name() :: atom().
-type func_name() :: atom().
-type test_name() ::
    init_per_suite
    | end_per_suite
    | {init_per_group, group_name()}
    | {end_per_group, group_name()}
    | {func_name(), group_name()}
    | func_name().
-type config() :: ct_suite:ct_config().

-type hook() :: {atom(), id()}.
-type state() :: #{
    hooks := [hook()],
    states := #{id() => hook_state()}
}.

-type part() ::
    init_per_suite
    | init_per_group
    | init_per_testcase
    | end_per_suite
    | end_per_group
    | end_per_testcase
    | on_tc_fail
    | on_tc_skip.

-type post_hook_call() ::
    post_init_per_suite
    | post_init_per_group
    | post_init_per_testcase
    | post_end_per_suite
    | post_end_per_group
    | post_end_per_testcase.

-type pre_hook_call() ::
    pre_init_per_suite
    | pre_init_per_group
    | pre_init_per_testcase
    | pre_end_per_suite
    | pre_end_per_group
    | pre_end_per_testcase.

-type hook_level() ::
    suite
    | group
    | testcase.

-type hook_response() ::
    [config()]
    | {skip, term()}
    | {fail, term()}.

-type hook_config() ::
    module()
    | {module(), Options :: [term()]}
    | {module(), Options :: [term()], Priority :: integer()}.

%%--------------------------------------------------------------------
%%% API

-spec set_state(id(), hook_state()) -> ok.
set_state(Id, State) ->
    ok = gen_server:call(?MODULE, {set_state, Id, State}).

-spec get_state(id()) -> {ok, hook_state()} | {error, {not_found, list()}}.
get_state(Id) ->
    case gen_server:call(?MODULE, {get_state, Id}) of
        {ok, State} -> {ok, State};
        Error = {error, {not_found, Details}} when is_list(Details) -> Error
    end.

-spec wrap(part(), [atom()], Fun) -> fun((Config) -> Res) when
    Fun :: fun((Config) -> Res) | fun((GroupOrTest :: atom(), Config) -> Res),
    Config :: config().
wrap(Part, Path, Fun) ->
    WrappedFun = gen_server:call(?MODULE, {wrap, Part, Fun}),
    %% apply path as closure
    fun(Config) -> WrappedFun(Path, Config) end.

-spec get_hooks() -> [module()].
get_hooks() ->
    [get_hook_module(Hook) || Hook <- get_hooks_config()].

-doc """
Starts the server within supervision tree
""".
-spec start_monitor() -> gen_server:start_mon_ret().
start_monitor() ->
    gen_server:start_monitor({local, ?MODULE}, ?MODULE, [], []).

%%--------------------------------------------------------------------
%%% gen_server callbacks

-spec init([]) -> {ok, state()}.
init([]) ->
    {ok, initialize_hooks()}.

-spec handle_call
    ({get_state, id()}, gen_server:from(), state()) ->
        {reply, {ok, hook_state()}, state()} | {error, {not_found, list()}};
    ({set_state, id(), hook_state()}, gen_server:from(), state()) -> {reply, ok, state()};
    ({wrap, part(), fun()}, gen_server:from(), state()) -> {reply, fun(([atom()], [config()]) -> term()), state()}.
handle_call({get_state, Id}, _From, State = #{states := HookStates}) ->
    case HookStates of
        #{Id := HookState} -> {reply, {ok, HookState}, State};
        _ -> {error, {not_found, [{state, State}, {id, Id}]}}
    end;
handle_call({set_state, Id, HookState}, _From, State = #{states := HookStates}) ->
    {reply, ok, State#{states => HookStates#{Id => HookState}}};
handle_call({wrap, Part, Fun}, _From, State) ->
    {reply, wrap_part(Part, Fun, State), State}.

-spec handle_cast(Request :: term(), State :: state()) -> no_return().
handle_cast(_Request, _State) ->
    error(badarg).

%%--------------------------------------------------------------------
%%% Internal functions

-spec initialize_hooks() -> state().
initialize_hooks() ->
    ConfiguredHooks = get_hooks_config(),
    NormalizedConfiguredHooks = [
        {get_hook_module(Hook), get_hook_opts(Hook), get_hook_priority(Hook)}
     || Hook <- ConfiguredHooks
    ],
    %% first we need the Id
    HooksWithId = [
        case Prio of
            undefined -> {0, wrapped_id(Mod, Opts), Mod, Opts, Prio};
            _ -> {Prio, wrapped_id(Mod, Opts), Mod, Opts, Prio}
        end
     || {Mod, Opts, Prio} <- NormalizedConfiguredHooks
    ],
    %% according to documentation, if two hooks have the same ID, the latter one gets dropped
    PreInitHooks0 = lists:ukeysort(2, HooksWithId),
    %% now sort with configured prio the inits (default prio being 0)
    PreInitHooks1 = lists:keysort(1, PreInitHooks0),

    %% now let's run the inits in order and build the state
    {States, HooksWithPriority} = lists:foldl(
        fun({_InitPrio, Id, Mod, Opts, ConfiguredPrio}, {StatesAcc, HooksAcc}) ->
            {Priority, HookState} = wrapped_init({Mod, Id}, Opts, ConfiguredPrio),
            {StatesAcc#{Id => HookState}, [{Priority, {Mod, Id}} | HooksAcc]}
        end,
        {#{}, []},
        PreInitHooks1
    ),

    %% sort hooks according to priority
    %% Note: This is reverse order for the priorities, but since we wrap, we want to wrap the
    %%       lowest priority hook first.
    SortedHooks = lists:keysort(1, HooksWithPriority),

    #{
        states => States,
        hooks => [Hook || {_Priority, Hook} <- SortedHooks]
    }.

-spec get_hooks_config() -> [hook_config()].
get_hooks_config() ->
    application:get_env(test_exec, ct_daemon_hooks, []) ++
        proplists:get_value(ct_hooks, application:get_env(test_exec, daemon_options, []), []).

-spec wrap_part(part(), fun(), state()) -> fun(([atom()], [config()]) -> term()).
wrap_part(Part, Fun, State) ->
    wrap_init_end(Part, Fun, State).

-spec wrap_init_end(part(), fun(), state()) -> fun(([atom()], [config()]) -> term()).
wrap_init_end(Part, Fun, #{hooks := HooksInInstallationOrder}) ->
    %% NOTE ON EXECUTION ORDER:
    %%
    %% As of OTP/26 CT's behaviour according to [https://www.erlang.org/doc/apps/common_test/ct_hooks_chapter#cth-execution-order]:
    %% > By default, each CTH installed is executed in the order that they are installed for init calls,
    %% > and then reversed for end calls. This is not always desired, so Common Test allows the user to specify
    %% > a priority for each hook.
    %%
    %% Implicit here is:
    %% - pre_init and post_init functions are executed in the same order
    %% - the hook with the "highest numerical priority" will be the first to run pre_init_per xxxx
    %%
    %% Starting from OTP/27, CT adds a new option ct_hooks_order option. The behaviour above is called `test`, and a
    %% new behaviour called `config` will be added, in which the order of the post_xxxx functions is reversed wrt pre_xxxx
    %% (see [https://github.com/erlang/otp/issues/7397] for discussion, and [https://github.com/erlang/otp/pull/7496]
    %% for the upcoming ct_hooks_order option).
    %%
    %% Here we implement only the behaviour that corresponds to the new `config` option.

    %% NB. we use a foldr to ensure that the first hook in HookInInstallationOrder is the innermost, so the first one to
    %% be executed
    WrappedWithPreAndPost = lists:foldr(
        fun(Hook, FunToWrap) ->
            fun(FullPathArg, ConfigArg0) ->
                PathArg =
                    case level(Part) of
                        testcase ->
                            [Suite | _] = FullPathArg,
                            [Suite, lists:last(FullPathArg)];
                        _ ->
                            FullPathArg
                    end,
                case call_if_exists_with_fallback_store_state(Hook, pre(Part), PathArg ++ [ConfigArg0], ok) of
                    {skip, SkipReason} ->
                        {skip, SkipReason};
                    {fail, FailReason} ->
                        {fail, FailReason};
                    HookCallbackResult ->
                        ConfigArg1 =
                            case is_list(HookCallbackResult) of
                                true ->
                                    HookCallbackResult;
                                false ->
                                    %% NB. If pre(Part) is not defined in the hook we get 'ok'
                                    ConfigArg0
                            end,
                        %% first step of error handling for the post functions where we set tc_status
                        {PostConfig, Return} =
                            try FunToWrap(PathArg, ConfigArg1) of
                                {skip, SkipReason} ->
                                    {
                                        [
                                            {tc_status, {skipped, SkipReason}}
                                            | lists:keydelete(tc_status, 1, ConfigArg1)
                                        ],
                                        {skip, SkipReason}
                                    };
                                {fail, FailReason} ->
                                    {
                                        [{tc_status, {failed, FailReason}} | lists:keydelete(tc_status, 1, ConfigArg1)],
                                        {fail, FailReason}
                                    };
                                OkResult ->
                                    ConfigArg2 =
                                        case init_or_end(Part) of
                                            init when is_list(OkResult) ->
                                                OkResult;
                                            _ ->
                                                ConfigArg1
                                        end,
                                    case proplists:is_defined(tc_status, ConfigArg2) of
                                        true -> {ConfigArg2, OkResult};
                                        false -> {[{tc_status, ok} | ConfigArg2], OkResult}
                                    end
                            catch
                                Class:Reason:Stacktrace ->
                                    Error = format_ct_error(Class, Reason, Stacktrace),
                                    {[{tc_status, {failed, Error}} | ConfigArg1], Error}
                            end,
                        Args = PathArg ++ [PostConfig, Return],
                        call_if_exists_with_fallback_store_state(Hook, post(Part), Args, Return)
                end
            end
        end,
        normalize_part(Part, Fun),
        HooksInInstallationOrder
    ),
    %% after the post_per functions we need to handle now failures, and call either on_tc_fail or on_tc_skip
    fun(PathArg, ConfigArg) ->
        [Suite | _] = PathArg,
        Result =
            try WrappedWithPreAndPost(PathArg, ConfigArg) of
                {skip, SkipReason} ->
                    {skip, SkipReason};
                {fail, FailReason} ->
                    {fail, FailReason};
                MaybeConfig ->
                    case init_or_end(Part) of
                        'end' ->
                            %% ends may return any kind of value
                            {ok, ConfigArg};
                        init ->
                            case proplists:get_value(tc_status, MaybeConfig, ok) of
                                ok ->
                                    {ok, lists:keydelete(tc_status, 1, MaybeConfig)};
                                FailOrSkip ->
                                    FailOrSkip
                            end
                    end
            catch
                Class:Reason:Stacktrace -> {fail, {'EXIT', {{Class, Reason}, Stacktrace}}}
            end,
        handle_post_result(HooksInInstallationOrder, build_test_name(Part, PathArg), Suite, Result)
    end.

-spec handle_post_result(Hooks, TestName, Suite, Result) ->
    hook_response()
when
    Hooks :: [hook()],
    TestName :: test_name(),
    Suite :: module(),
    Result :: {ok, [config()]} | {skip, term()} | {fail, term()}.
handle_post_result(Hooks, TestName, Suite, Result) ->
    ReverseHooks = lists:reverse(Hooks),
    case Result of
        {skip, SkipReason} ->
            [
                call_if_exists_with_fallback_store_state(
                    Hook, on_tc_skip, [Suite, TestName, {tc_user_skip, SkipReason}], ok
                )
             || Hook <- ReverseHooks
            ],
            {skip, SkipReason};
        {fail, FailReason} ->
            [
                call_if_exists_with_fallback_store_state(
                    Hook, on_tc_fail, [Suite, TestName, FailReason], ok
                )
             || Hook <- ReverseHooks
            ],
            {fail, FailReason};
        {ok, Config} ->
            case lists:keyfind(tc_status, 1, Config) of
                false ->
                    Config;
                {tc_status, {skipped, SkipReason}} ->
                    [
                        call_if_exists_with_fallback_store_state(
                            Hook, on_tc_skip, [Suite, TestName, {tc_user_skip, SkipReason}], ok
                        )
                     || Hook <- ReverseHooks
                    ],
                    {skip, SkipReason};
                {tc_status, {failed, FailReason}} ->
                    [
                        call_if_exists_with_fallback_store_state(
                            Hook, on_tc_fail, [Suite, TestName, FailReason], ok
                        )
                     || Hook <- ReverseHooks
                    ],
                    {fail, FailReason}
            end
    end.

-spec format_ct_error(throw | error | exit, Reason, Stacktrace) ->
    {fail, {thrown, Reason, Stacktrace}}
    | {fail, {Reason, Stacktrace}}
    | {fail, Reason}
when
    Reason :: term(), Stacktrace :: erlang:stacktrace().
format_ct_error(throw, Reason, Stacktrace) ->
    {fail, {thrown, Reason, Stacktrace}};
format_ct_error(error, Reason, Stacktrace) ->
    {fail, {Reason, Stacktrace}};
format_ct_error(exit, Reason, Stacktrace) when is_list(Stacktrace) ->
    {fail, {exit, Reason, Stacktrace}}.

-spec build_test_name(part(), [atom()]) -> test_name().
build_test_name(init_per_suite, _Path) ->
    init_per_suite;
build_test_name(end_per_suite, _Path) ->
    end_per_suite;
build_test_name(init_per_group, [_, Group]) ->
    {init_per_group, Group};
build_test_name(end_per_group, [_, Group]) ->
    {end_per_group, Group};
build_test_name(init_per_testcase, [_, Test]) ->
    Test;
build_test_name(init_per_testcase, Path) ->
    [Test, Group | _] = lists:reverse(Path),
    {Group, Test};
build_test_name(end_per_testcase, [_, Test]) ->
    Test;
build_test_name(end_per_testcase, Path) ->
    [Test, Group | _] = lists:reverse(Path),
    {Group, Test}.

-spec get_hook_module(hook_config()) -> module().
get_hook_module({Mod, _, _}) -> Mod;
get_hook_module({Mod, _}) -> Mod;
get_hook_module(Mod) -> Mod.

-spec get_hook_opts(hook_config()) -> [term()].
get_hook_opts({_, Opts, _}) -> Opts;
get_hook_opts({_, Opts}) -> Opts;
get_hook_opts(_) -> [].

-spec get_hook_priority(hook_config()) -> integer() | undefined.
get_hook_priority({_, _, Prio}) -> Prio;
get_hook_priority(_) -> undefined.

-spec normalize_part(part(), fun()) -> fun().
normalize_part(Part, Fun) ->
    SafeFun = get_safe_part(Part, Fun),
    case level(Part) of
        suite -> fun([_Suite], Config) -> SafeFun(Config) end;
        group -> fun([_Suite, Group], Config) -> SafeFun(Group, Config) end;
        testcase -> fun(Path, Config) -> SafeFun(lists:last(Path), Config) end
    end.

%% wrappers because most calls are optional
-spec call_if_exists(module(), atom(), [term()], Default :: {'$lazy', LazyFun :: fun(() -> term())} | term()) -> term().
call_if_exists(Mod, Fun, Args, Default) ->
    case erlang:function_exported(Mod, Fun, erlang:length(Args)) of
        true ->
            erlang:apply(Mod, Fun, Args);
        false ->
            case Default of
                {'$lazy', LazyFun} when is_function(LazyFun, 0) -> LazyFun();
                _ -> Default
            end
    end.

-spec call_if_exists_with_fallback(module(), atom(), [term()], term()) -> term().
call_if_exists_with_fallback(Mod, Fun, Args, ReturnDefault) ->
    [_ | FallbackArgs] = Args,
    call_if_exists(Mod, Fun, Args, {'$lazy', fun() -> call_if_exists(Mod, Fun, FallbackArgs, ReturnDefault) end}).

-spec call_if_exists_with_fallback_store_state({module(), term()}, atom(), [term()], term()) -> term().
call_if_exists_with_fallback_store_state({Mod, Id}, Fun, Args, ReturnDefault) ->
    {ok, State} = get_state(Id),
    Default =
        case Fun of
            _ when Fun =:= on_tc_fail orelse Fun =:= on_tc_skip -> State;
            _ -> {ReturnDefault, State}
        end,
    CallReturn = call_if_exists_with_fallback(Mod, Fun, Args ++ [State], Default),
    {NewReturn, NewState} =
        case Fun of
            _ when Fun =:= on_tc_fail orelse Fun =:= on_tc_skip -> {ok, CallReturn};
            _ -> CallReturn
        end,
    ok = set_state(Id, NewState),
    NewReturn.

-spec wrapped_id(module(), opts()) -> term().
wrapped_id(Mod, Opts) ->
    case code:ensure_loaded(Mod) of
        {module, Mod} -> ok;
        Error -> error({load_hooks_module, Mod, Error})
    end,
    call_if_exists(Mod, id, [Opts], make_ref()).

-spec wrapped_init(hook(), opts(), integer() | undefined) -> {integer(), hook_state()}.
wrapped_init({Mod, Id}, Opts, ConfiguredPriority) ->
    {InitPriority, InitState} =
        case Mod:init(Id, Opts) of
            {ok, State} -> {0, State};
            {ok, State, Priority} -> {Priority, State};
            Error -> error({hooks_init_error, Error})
        end,
    case ConfiguredPriority of
        undefined -> {InitPriority, InitState};
        _ -> {ConfiguredPriority, InitState}
    end.

-spec pre(part()) -> pre_hook_call().
pre(init_per_suite) -> pre_init_per_suite;
pre(init_per_group) -> pre_init_per_group;
pre(init_per_testcase) -> pre_init_per_testcase;
pre(end_per_suite) -> pre_end_per_suite;
pre(end_per_group) -> pre_end_per_group;
pre(end_per_testcase) -> pre_end_per_testcase.

-spec post(part()) -> post_hook_call().
post(init_per_suite) -> post_init_per_suite;
post(init_per_group) -> post_init_per_group;
post(init_per_testcase) -> post_init_per_testcase;
post(end_per_suite) -> post_end_per_suite;
post(end_per_group) -> post_end_per_group;
post(end_per_testcase) -> post_end_per_testcase.

-spec level(part()) -> hook_level().
level(init_per_suite) -> suite;
level(init_per_group) -> group;
level(init_per_testcase) -> testcase;
level(end_per_suite) -> suite;
level(end_per_group) -> group;
level(end_per_testcase) -> testcase.

-spec init_or_end(part()) -> init | 'end'.
init_or_end(init_per_suite) -> init;
init_or_end(init_per_group) -> init;
init_or_end(init_per_testcase) -> init;
init_or_end(end_per_suite) -> 'end';
init_or_end(end_per_group) -> 'end';
init_or_end(end_per_testcase) -> 'end'.

-spec get_safe_part(part(), fun()) -> fun().
get_safe_part(Part, Fun) ->
    case is_exported(Fun) of
        true -> Fun;
        false -> dummy(Part)
    end.

-spec dummy(part()) -> fun().
dummy(init_per_suite) -> fun(Config) -> Config end;
dummy(init_per_group) -> fun(_, Config) -> Config end;
dummy(init_per_testcase) -> fun(_, Config) -> Config end;
dummy(end_per_suite) -> fun(_) -> ok end;
dummy(end_per_group) -> fun(_, _) -> ok end;
dummy(end_per_testcase) -> fun(_, _) -> ok end.

-spec is_exported(fun()) -> boolean().
is_exported(Fun) ->
    case maps:from_list(erlang:fun_info(Fun)) of
        #{
            type := external,
            module := Module,
            name := Function,
            arity := Arity
        } when is_atom(Module), is_atom(Function), is_integer(Arity) ->
            erlang:function_exported(Module, Function, Arity);
        _ ->
            false
    end.
