%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(buck_ct_provider).
-moduledoc """
behavior module defining callbacks for buck2 test providers
""".
-compile(warn_missing_spec_all).

-export([
    do_init/2,
    do_pre_listing/1,
    do_post_listing/2,
    do_pre_running/1,
    do_post_running/1,
    do_terminate/1
]).
-optional_callbacks([
    init/2, pre_listing/1, post_listing/2, pre_running/1, post_running/1, terminate/1
]).

-export_type([test_names/0]).

-include("buck_ct_records.hrl").
-include_lib("kernel/include/logger.hrl").

-type test_names() :: [binary()].

-type state() :: term().

-type return_type() :: {ok, state()} | {error, term()}.

-type return_type(Result) :: {ok, Result, state()} | {error, term()}.

-type buck_ct_provider() :: {atom(), state()}.

-type init_argument_type() :: term().

% ------------------- Behaviors Callbacks -------------------------

%%% Initialize the state of the provider
-callback init(init_argument_type(), #init_provider_state{}) -> return_type().

%%% Executed before listing and updates the state of the provider
-callback pre_listing(state()) -> return_type().

%%% Executed after listing and updates the tests and state of the provider
-callback post_listing(TestNames, State) -> return_type(test_names()) when
    TestNames :: test_names(),
    State :: state().

%%% Executed before running the tests and updates the state of the provider
-callback pre_running(state()) -> return_type().

%%% Executed after running the tests and updates the state of the provider
-callback post_running(state()) -> return_type().

%%% Executed as closing.
-callback terminate(state()) -> return_type().

% ------------------- Exported Methods -------------------------

-doc """
Handles calling the init method on providers.
Calls the init method if this one is present and updates the state
""".
-spec do_init({atom(), init_argument_type()}, #init_provider_state{}) -> buck_ct_provider().
do_init({ProviderName, Args}, InitState) ->
    execute_method_on_provider(init, ProviderName, InitState, [Args]).

-doc """
Handles calling the pre_listing method on providers.
Calls the pre_listing method if this one is present and updates the state.
""".
-spec do_pre_listing(buck_ct_provider()) -> buck_ct_provider().
do_pre_listing({ProviderName, ProviderState}) ->
    execute_method_on_provider(pre_listing, ProviderName, ProviderState, []).

-doc """
Handles calling the post_listing method on providers.
Calls the post_listing method if this one is present and updates the state.
""".
-spec do_post_listing({ProviderName, ProviderState}, TestNames) -> {buck_ct_provider(), test_names()} when
    ProviderName :: module(),
    ProviderState :: state(),
    TestNames :: test_names().
do_post_listing({ProviderName, ProviderState}, TestNames) ->
    Result = execute_method_on_provider_with_result(
        post_listing, ProviderName, ProviderState, [TestNames], {ok, TestNames, ProviderState}
    ),
    case Result of
        {ok, FilteredTestNames, NewState} when is_list(FilteredTestNames) ->
            KnownTestNames = maps:from_keys(TestNames, true),
            ValidatedTestNames = validate_test_names(ProviderName, FilteredTestNames, KnownTestNames),
            {{ProviderName, NewState}, ValidatedTestNames};
        OtherReturn ->
            error({invalid_post_listing_return, ProviderName, OtherReturn, ~"expected {ok, [binary()], State}"})
    end.

-doc """
Handles calling the pre_running method on providers.
Calls the pre_running method if this one is present and updates the state.
""".
-spec do_pre_running(buck_ct_provider()) -> buck_ct_provider().
do_pre_running({ProviderName, ProviderState}) ->
    execute_method_on_provider(pre_running, ProviderName, ProviderState, []).

-doc """
Handles calling the post_running method on providers.
Calls the post_running method if this one is present and updates the state
""".
-spec do_post_running(buck_ct_provider()) -> buck_ct_provider().
do_post_running({ProviderName, ProviderState}) ->
    execute_method_on_provider(post_running, ProviderName, ProviderState, []).

-doc """
Handles calling the terminate method on providers.
Calls the terminate method if this one is present and updates the state.
""".
-spec do_terminate(buck_ct_provider()) -> buck_ct_provider().
do_terminate({ProviderName, ProviderState}) ->
    execute_method_on_provider(terminate, ProviderName, ProviderState, []).

% ------------------- Helpers Methods -------------------------

-spec validate_test_names(ProviderName, TestNames, KnownTestNames) -> test_names() when
    ProviderName :: module(),
    TestNames :: [term()],
    KnownTestNames :: #{binary() => true}.
validate_test_names(_ProviderName, [], _KnownTestNames) ->
    [];
validate_test_names(ProviderName, [TestName | TestNames], KnownTestNames) when
    is_binary(TestName), is_map_key(TestName, KnownTestNames)
->
    [TestName | validate_test_names(ProviderName, TestNames, KnownTestNames)];
validate_test_names(ProviderName, [TestName | _TestNames], _KnownTestNames) when is_binary(TestName) ->
    error({unknown_test_name_from_provider, ProviderName, TestName});
validate_test_names(ProviderName, [TestName | _TestNames], _KnownTestNames) ->
    error({invalid_test_name_from_provider, ProviderName, TestName, ~"expected a binary"}).

-doc """
Handles the execution of the method on a provider and dealing with the result.
Skip if the method is undefined, updates the state or crash according to the return result
of the method.
""".
-spec execute_method_on_provider(Method, ProviderName, ProviderState, Args) -> buck_ct_provider() when
    Method :: init | pre_listing | pre_running | post_running | terminate,
    ProviderName :: module(),
    ProviderState :: state(),
    Args :: [term()].
execute_method_on_provider(Method, ProviderName, ProviderState, Args) ->
    DefaultState =
        case Method of
            init -> #{};
            _ -> ProviderState
        end,
    Result = execute_method_on_provider_with_result(Method, ProviderName, ProviderState, Args, {ok, DefaultState}),
    case Result of
        {ok, NewState} ->
            {ProviderName, NewState};
        OtherReturn ->
            case Method of
                terminate ->
                    {ProviderName, ProviderState};
                _ ->
                    ?LOG_DEBUG(
                        "Method ~tp on provider ~tp with state ~tp ~n returned with an unexpected return ~tp ~n",
                        [
                            Method, ProviderName, ProviderState, OtherReturn
                        ]
                    ),
                    {ProviderName, ProviderState}
            end
    end.

-spec execute_method_on_provider_with_result(Method, ProviderName, ProviderState, Args, DefaultResult) -> term() when
    Method :: atom(),
    ProviderName :: module(),
    ProviderState :: state(),
    Args :: [term()],
    DefaultResult :: term().
execute_method_on_provider_with_result(Method, ProviderName, ProviderState, Args, DefaultResult) ->
    try safely_execute(ProviderName, Method, Args ++ [ProviderState]) of
        undefined_method ->
            DefaultResult;
        {error, Reason} ->
            ErrorMsg = unicode:characters_to_list(
                io_lib:format(
                    "Method ~tp on provider ~tp with state ~tp ~n returned with error ~tp ~n", [
                        Method, ProviderName, ProviderState, Reason
                    ]
                )
            ),
            ?LOG_ERROR(ErrorMsg),
            throw({error_in_provider, {ProviderName, Reason}});
        Result ->
            Result
    catch
        Class:Reason:StackTrace ->
            ErrorMsg = unicode:characters_to_list(
                io_lib:format("Method ~tp on provider ~tp with state ~tp ~n ~ts ~n", [
                    Method,
                    ProviderName,
                    ProviderState,
                    erl_error:format_exception(Class, Reason, StackTrace)
                ])
            ),
            ?LOG_ERROR(ErrorMsg),
            throw({crash_in_provider, {Class, Reason, StackTrace}})
    end.

-doc """
Executes the method if this one is exported, otherwise return undefined_method.
""".
-spec safely_execute(atom(), atom(), [term()]) -> undefined_method | term().
safely_execute(Module, Method, Args) ->
    Module:module_info(),
    case erlang:function_exported(Module, Method, length(Args)) of
        false ->
            undefined_method;
        true ->
            apply(Module, Method, Args)
    end.
