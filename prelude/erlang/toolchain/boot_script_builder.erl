%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(boot_script_builder).
-author("loscher@fb.com").
-moduledoc """
Build the release resource file, and boot scripts from a given spec file. The spec file format
is defined in erlang_release.bzl

usage:
  boot_script_builder.escript release_spec.term output_dir
""".

-export([main/1]).

-define(EXITSUCCESS, 0).
-define(EXITERROR, 1).

-spec main([string()]) -> ok.
main([ReleaseSpec, OutDir]) ->
    try
        do(ReleaseSpec, OutDir),
        erlang:halt(?EXITSUCCESS)
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~ts:~ts~n", [Type, Reason]),
            erlang:halt(?EXITERROR)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("boot_script_builder.escript release_spec.term output_dir~n").

do(SpecFile, OutDir) ->
    {ok, SpecFileContents} = file:read_file(SpecFile, [raw]),
    #{
        <<"apps">> := Apps,
        <<"lib_dir">> := LibDir,
        <<"name">> := RelNameBin,
        <<"version">> := RelVersionBin
    } = json:decode(SpecFileContents),
    OTPAppMapping = get_otp_apps_mapping(),
    LibDirWildcard = binary_to_list(filename:absname(filename:join([LibDir, "*", "ebin"]))),

    %% systools requires us to be in the same directory as the .rel file
    %% it also magically discovers the release root and makes all paths
    %% relative to it if the structure is OTP compliant
    ok = filelib:ensure_path(OutDir),
    ok = file:set_cwd(OutDir),

    RelName = binary_to_list(RelNameBin),
    RelVersion = binary_to_list(RelVersionBin),
    build_no_dot_erlang_boot(Apps, [LibDirWildcard], RelName, RelVersion, OTPAppMapping),
    build_start_boot(Apps, [LibDirWildcard], RelName, RelVersion, OTPAppMapping).

build_no_dot_erlang_boot(
    Apps,
    PathOption,
    RelName,
    RelVersion,
    OTPAppMapping
) ->
    {OTPApps, _Others} = lists:partition(
        fun(#{<<"resolved">> := Resolved}) -> not Resolved end,
        Apps
    ),

    % Extract kernel version from OTPApps
    Kernel =
        case
            lists:search(
                fun(#{<<"name">> := Name}) -> Name =:= <<"kernel">> end,
                OTPApps
            )
        of
            {value, _} ->
                #{<<"kernel">> := #{version := KernelVersion}} = OTPAppMapping,
                {kernel, binary_to_list(KernelVersion)};
            false ->
                error({abort, "kernel version not found in OTPApps"})
        end,

    % Extract stdlib version from OTPApps
    StdLib =
        case
            lists:search(
                fun(#{<<"name">> := Name}) -> Name =:= <<"stdlib">> end,
                OTPApps
            )
        of
            {value, _} ->
                #{<<"stdlib">> := #{version := StdLibVersion}} = OTPAppMapping,
                {stdlib, binary_to_list(StdLibVersion)};
            false ->
                error({abort, "stdlib version not found in OTPApps"})
        end,

    RelFileContent = {
        release,
        {RelName, RelVersion},
        {erts, erlang:system_info(version)},
        [Kernel, StdLib]
    },

    make_boot_script(RelName, RelFileContent, [
        no_warn_sasl,
        no_dot_erlang,
        {script_name, "no_dot_erlang"},
        {path, PathOption}
    ]).

build_start_boot(
    Apps,
    PathOption,
    RelName,
    RelVersion,
    OTPAppMapping
) ->
    %% OTP apps dependencies are not captures and we need to calculate
    %% them first
    {OTPApps, Others} = lists:partition(
        fun(#{<<"resolved">> := Resolved}) -> not Resolved end,
        Apps
    ),
    OTPAppDeps = get_otp_app_deps(OTPApps, OTPAppMapping),

    %% rel file content
    RelFileContent = {
        release,
        {RelName, RelVersion},
        {erts, erlang:system_info(version)},
        [
            case StartType of
                <<"permanent">> -> {binary_to_atom(AppName), binary_to_list(AppVersion)};
                <<"load">> -> {binary_to_atom(AppName), binary_to_list(AppVersion), load}
            end
         || #{
                <<"name">> := AppName,
                <<"version">> := AppVersion,
                <<"type">> := StartType
            } <- OTPAppDeps ++ Others
        ]
    },
    make_boot_script(RelName, RelFileContent, [
        no_warn_sasl,
        no_dot_erlang,
        {script_name, "start"},
        {path, PathOption}
    ]).

make_boot_script(RelName, RelFileContent, Options) ->
    ok = write_file(
        io_lib:format("~ts.rel", [RelName]),
        io_lib:format("~tp.", [RelFileContent])
    ),
    case systools:make_script(RelName, Options) of
        ok ->
            ok;
        Error ->
            error({abort, Error})
    end.

get_otp_apps_mapping() ->
    lists:foldl(
        fun(Path, Acc) ->
            case parse_app_path(Path) of
                skip ->
                    Acc;
                [AppName, AppVersion] ->
                    Acc#{
                        list_to_binary(AppName) => #{
                            version => list_to_binary(AppVersion),
                            app_file => app_file_path(Path, AppName)
                        }
                    }
            end
        end,
        #{},
        code:get_path()
    ).

get_otp_app_deps(OTPApps, OTPAppMapping) ->
    InitialDeps = [
        Dependency
     || #{<<"name">> := AppName} <- OTPApps,
        Dependency <- begin
            #{AppName := #{app_file := AppFile, version := AppVersion}} = OTPAppMapping,
            #{name := AppName, version := AppVersion, deps := Dependencies} = parse_app_file(
                AppFile
            ),
            Dependencies
        end
    ],

    InitialAcc = lists:foldl(
        fun(UnVersionedSpec = #{<<"name">> := AppName}, Acc) ->
            % get specific version from toolchain
            #{AppName := #{version := AppVersion}} = OTPAppMapping,
            % replace dynamic version with specific one
            VersionedSpec = UnVersionedSpec#{<<"version">> => AppVersion},
            Acc#{binary_to_atom(AppName) => VersionedSpec}
        end,
        #{},
        OTPApps
    ),

    get_otp_app_deps(InitialDeps, OTPAppMapping, InitialAcc).

get_otp_app_deps([], _, Acc) ->
    maps:values(Acc);
get_otp_app_deps([App | Rest], OTPAppMapping, Acc) ->
    AppName = atom_to_binary(App),
    #{AppName := #{app_file := AppFile, version := AppVersion}} = OTPAppMapping,
    #{name := AppName, version := AppVersion, deps := Dependencies} = parse_app_file(AppFile),
    Spec = #{
        <<"name">> => AppName,
        <<"version">> => AppVersion,
        <<"type">> => <<"permanent">>
    },
    FilteredDependencies = [Dep || Dep <- Dependencies, not is_map_key(Dep, Acc)],
    get_otp_app_deps(Rest ++ FilteredDependencies, OTPAppMapping, Acc#{App => Spec}).

app_file_path(Path, Name) ->
    AppFile = unicode:characters_to_list(io_lib:format("~ts.app", [Name])),
    filename:join(Path, AppFile).

parse_app_path(Path) ->
    case string:prefix(Path, [code:lib_dir(), "/"]) of
        nomatch ->
            skip;
        RelPath ->
            case filename:split(RelPath) of
                [AppFolder, "ebin"] ->
                    string:split(AppFolder, "-");
                _ ->
                    skip
            end
    end.

parse_app_file(File) ->
    io:format("~ts~n", [File]),
    {ok, [{application, App, Props}]} =
        file:consult(File),
    AppName = atom_to_binary(App),
    Version = list_to_binary(proplists:get_value(vsn, Props)),
    Dependencies =
        proplists:get_value(applications, Props, []) ++
            proplists:get_value(included_applications, Props, []),
    #{
        name => AppName,
        version => Version,
        deps => Dependencies
    }.

-spec write_file(file:name_all() | iodata(), iolist()) -> string().
write_file(File, Data) ->
    case
        % We write in raw mode because this is a standalone escript, so we don't
        % need advanced file server features, and we gain performance by avoiding
        % calling through the file server
        file:open(File, [write, binary, raw])
    of
        {ok, Handle} ->
            try
                % We use file:pwrite instead of file:write_file to work around
                % the latter needlessly flattening iolists (as returned by
                % json:encode/1, etc.) to a binary
                file:pwrite(Handle, 0, Data)
            after
                file:close(Handle)
            end,
            ok;
        {error, _} = Error ->
            Error
    end.
