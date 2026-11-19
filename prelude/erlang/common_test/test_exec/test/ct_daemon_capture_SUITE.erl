%% Copyright (c) Meta Platforms, Inc. and affiliates.
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%%% % @format
-module(ct_daemon_capture_SUITE).
-typing([eqwalizer]).

-export([all/0, init_per_testcase/2, end_per_testcase/2]).
-export([
    test_start_stop/1,
    test_capture_via_call/1,
    test_capture_via_message/1,
    test_put_chars_without_capture/1,
    test_put_chars_with_capture/1,
    test_io_request_forwarding/1,
    test_unknown_request/1
]).

% elp:ignore WA003 (better_assertions) - Open Source
-include_lib("stdlib/include/assert.hrl").

all() ->
    [
        test_start_stop,
        test_capture_via_call,
        test_capture_via_message,
        test_put_chars_without_capture,
        test_put_chars_with_capture,
        test_io_request_forwarding,
        test_unknown_request
    ].

init_per_testcase(_TestCase, Config) ->
    {ok, CaptureGL} = ct_daemon_capture:start_link(erlang:group_leader()),
    [{capture_gl, CaptureGL} | Config].

end_per_testcase(_TestCase, Config) ->
    ct_daemon_capture:stop(proplists:get_value(capture_gl, Config)),
    ok.

%% Test that start_link and stop work correctly
test_start_stop(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    ?assert(is_pid(CaptureGL)),
    ?assert(is_process_alive(CaptureGL)).

%% Test capture enable/disable via gen_server:call (testshell's approach)
test_capture_via_call(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    ?assertEqual(ok, gen_server:call(CaptureGL, {capture, self()})),
    ?assertEqual(ok, gen_server:call(CaptureGL, {capture, false})).

%% Test capture enable/disable via message (OTP's approach)
test_capture_via_message(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    CaptureGL ! {capture, self()},
    %% Synchronize by making a call to ensure the message was processed
    _ = sys:get_state(CaptureGL),
    CaptureGL ! {capture, false},
    _ = sys:get_state(CaptureGL),
    ?assert(is_process_alive(CaptureGL)).

%% Test put_chars without capture enabled
test_put_chars_without_capture(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    Ref = make_ref(),
    CaptureGL ! {io_request, self(), Ref, {put_chars, unicode, ~"test output"}},
    receive
        {io_reply, Ref, ok} -> ok
    after 1000 -> ?assert(false)
    end,
    %% No captured message should be received
    receive
        {captured, _} -> ?assert(false)
    after 100 -> ok
    end.

%% Test put_chars with capture enabled
test_put_chars_with_capture(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    ok = gen_server:call(CaptureGL, {capture, self()}),
    Ref = make_ref(),
    CaptureGL ! {io_request, self(), Ref, {put_chars, unicode, ~"captured output"}},
    %% Should receive both io_reply and captured message
    receive
        {io_reply, Ref, ok} -> ok
    after 1000 -> ?assert(false)
    end,
    receive
        {captured, "captured output"} -> ok;
        {captured, Other} -> ?assertEqual("captured output", Other)
    after 1000 ->
        ?assert(false)
    end.

%% Test that IO requests are forwarded to original group leader
test_io_request_forwarding(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    OldGL = erlang:group_leader(),
    true = erlang:group_leader(CaptureGL, self()),
    try
        io:format("test~n"),
        ok
    after
        erlang:group_leader(OldGL, self())
    end.

%% Test unknown request returns error
test_unknown_request(Config) ->
    CaptureGL = proplists:get_value(capture_gl, Config),
    ?assertEqual({error, unknown_request}, gen_server:call(CaptureGL, {unknown, request})).
