%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(listing_provider).

-export([init/2, pre_listing/1, post_listing/2, terminate/1]).
-export_type([args/0]).

-include_lib("common/include/buck_ct_records.hrl").

-type options() :: #{
    expected := buck_ct_provider:test_names(),
    filtered := buck_ct_provider:test_names(),
    output_file := string()
}.
-type args() :: [options()].
-type state() :: #{options := options(), output_dir := file:filename()}.
-type failure_state() ::
    error | crash | unexpected | invalid_list | invalid_test_name | unknown_test_name.

-spec init(Args, InitState) -> {ok, state()} when
    Args :: args(),
    InitState :: #init_provider_state{}.
init(Args, InitState) ->
    [Options] = Args,
    {ok, #{options => Options, output_dir => InitState#init_provider_state.output_dir}}.

-spec pre_listing(State) -> {ok, state() | updated} | unexpected when
    State :: state() | update | unexpected.
pre_listing(update) ->
    {ok, updated};
pre_listing(unexpected) ->
    unexpected;
pre_listing(State) ->
    {ok, State}.

-spec post_listing(TestNames, State) ->
    {ok, [binary() | invalid_test_name] | invalid_list, state() | failure_state()}
    | {error, test_error}
    | unexpected
when
    TestNames :: buck_ct_provider:test_names(),
    State :: state() | failure_state().
post_listing(_TestNames, error) ->
    {error, test_error};
post_listing(_TestNames, crash) ->
    error(test_crash);
post_listing(_TestNames, unexpected) ->
    unexpected;
post_listing(_TestNames, invalid_list) ->
    {ok, invalid_list, invalid_list};
post_listing(_TestNames, invalid_test_name) ->
    {ok, [invalid_test_name], invalid_test_name};
post_listing(_TestNames, unknown_test_name) ->
    {ok, [~".missing"], unknown_test_name};
post_listing(TestNames, State) when is_map(State) ->
    #{options := Options} = State,
    #{expected := TestNames, filtered := FilteredTestNames} = Options,
    {ok, FilteredTestNames, State#{options => Options#{expected => FilteredTestNames}}}.

-spec terminate(State) -> {ok, state()} when State :: state().
terminate(State) ->
    #{options := Options, output_dir := OutputDir} = State,
    #{expected := TestNames, output_file := OutputFile} = Options,
    ok = file:write_file(filename:join(OutputDir, OutputFile), term_to_binary(TestNames)),
    {ok, State}.
