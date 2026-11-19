%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.
%% % @format
-module(test_logger).
-compile(warn_missing_spec_all).

-export([set_up_logger/3, flush/0, get_std_out/2, get_log_file/2, configure_logger/1]).

-spec set_up_logger(LogDir, AppName, Type) -> ok when
    LogDir :: file:filename(),
    AppName :: atom(),
    Type :: capture_stdout | no_capture_stdout.
set_up_logger(LogDir, AppName, Type) ->
    Log = get_log_file(LogDir, AppName),
    filelib:ensure_dir(Log),
    [logger:update_handler_config(Id, level, none) || Id <- logger:get_handler_ids()],
    case Type of
        no_capture_stdout ->
            ok;
        capture_stdout ->
            StdOut = get_std_out(LogDir, AppName),
            filelib:ensure_dir(StdOut),
            {ok, LogFileOpened} = file:open(StdOut, [write, {encoding, utf8}]),
            test_artifact_directory:link_to_artifact_dir(StdOut, LogDir, fun artifact_annotations:default_annotation/1),
            group_leader(
                LogFileOpened, self()
            )
    end,
    configure_logger(Log).

-spec configure_logger(file:filename_all()) -> ok.
configure_logger(LogFile) ->
    ok = logger:set_primary_config(#{
        level => all,
        filter_default => log
    }),
    configure_logger_handler(LogFile, 3).

-spec configure_logger_handler(file:filename_all(), non_neg_integer()) -> ok.
configure_logger_handler(LogFile, Retries) ->
    Result = logger:add_handler(
        file_handler, logger_std_h, #{
            config => #{
                file => LogFile,
                filesync_repeat_interval => 100
            },
            filter_default => log,
            formatter =>
                {logger_formatter, #{
                    template => [
                        time,
                        " ",
                        pid,
                        {file, [" ", file], []},
                        {line, [":", line], []},
                        " == ",
                        level,
                        ": ",
                        msg,
                        "\n"
                    ]
                }}
        }
    ),
    case Result of
        ok ->
            ok;
        {error, {handler_not_added, file_ctrl_process_not_started}} when Retries > 0 ->
            %% Under host memory/IO pressure the OTP logger's hardcoded 5s
            %% file-ctrl spawn timeout can fire before I/O completes.
            io:format(
                standard_error,
                "Warning: logger file_ctrl_process not started, "
                "retrying (~b attempts left)~n",
                [Retries - 1]
            ),
            configure_logger_handler(LogFile, Retries - 1);
        {error, _Reason} = Error ->
            erlang:error(Error)
    end.

-spec flush() -> ok | {error, term()}.
flush() ->
    logger_std_h:filesync(file_handler).

-spec get_std_out(file:filename_all(), atom()) -> file:filename_all().
get_std_out(LogDir, AppName) ->
    filename:join(LogDir, io_lib:format("~tp.stdout.txt", [AppName])).

-spec get_log_file(file:filename_all(), atom()) -> file:filename_all().
get_log_file(LogDir, AppName) ->
    filename:join(LogDir, io_lib:format("~tp.log", [AppName])).
