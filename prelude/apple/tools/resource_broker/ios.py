# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import os
from dataclasses import dataclass
from typing import Optional

from packaging.version import Version

from .simctl_runtime import list_runtimes, XCSimRuntime
from .simulator import (
    managed_simulators_list_from_stdout,
    Simulator,
    SimulatorInfo,
    SimulatorState,
    SimulatorType,
)
from .timeouts import SIMULATOR_BOOT_TIMEOUT
from .utils import execute_generic_text_producing_command


@dataclass(frozen=True)
class SimulatorSpec:
    device: str
    os_version: str


def _device_set_path() -> str:
    return os.path.expanduser("~/Library/Developer/Buck2IdbDeviceSet")


def _list_managed_simulators_command(simulator_manager: str) -> list[str]:
    return [
        simulator_manager,
        "list",
        "--device-set-path",
        _device_set_path(),
        "--only",
        "simulator",
    ]


def _create_simulator_command(simulator_manager: str, sim_spec: str) -> list[str]:
    return [
        simulator_manager,
        "create",
        "--device-set-path",
        _device_set_path(),
        "--configuration",
        sim_spec,
    ]


def _boot_simulator_command(simulator_manager: str, udid: str) -> list[str]:
    return [
        simulator_manager,
        "boot",
        "--device-set-path",
        _device_set_path(),
        udid,
    ]


def _compatible_device_type_from_runtime(
    runtime: XCSimRuntime, simulator_type: SimulatorType, device: Optional[str]
) -> Optional[str]:
    device_types = list(runtime.supported_device_types)
    device_types = [
        device_type
        for device_type in device_types
        if simulator_type.matches_device_identifier(device_type.identifier)
    ]
    if device:
        if device.startswith("com.apple."):
            # filter by device type identifier
            device_types = [
                device_type
                for device_type in device_types
                if device_type.identifier == device
            ]
        else:
            # filter by device name
            device_types = [
                device_type
                for device_type in device_types
                if device_type.name == device
            ]
    if not device_types:
        return None
    selected_device = next(
        (
            device_type.name
            for device_type in device_types
            if device_type.name == simulator_type.default_device()
        ),
        device_types[0].name,
    )
    return selected_device


def _select_simulator_spec(
    runtimes: list[XCSimRuntime],
    simulator_type: SimulatorType,
    os_version: Optional[str],
    device: Optional[str],
) -> SimulatorSpec:
    runtimes = [
        runtime
        for runtime in runtimes
        if any(
            simulator_type.matches_device_identifier(device_type.identifier)
            for device_type in runtime.supported_device_types
        )
    ]
    runtimes.sort(key=lambda x: Version(x.version), reverse=True)

    if os_version:
        runtimes = [x for x in runtimes if x.name == os_version]
    for runtime in runtimes:
        device_type = _compatible_device_type_from_runtime(
            runtime=runtime, simulator_type=simulator_type, device=device
        )
        if device_type:
            return SimulatorSpec(device_type, f"{runtime.platform} {runtime.version}")
    raise RuntimeError(
        "No Xcode simctl compatible os runtime and device available. Try to `sudo xcode-select -s <path_to_xcode>` and *open Xcode to install all required components*."
    )


async def _generic_managed_simulators_list_command(
    name: str, cmd: list[str]
) -> list[Simulator]:
    stdout = await execute_generic_text_producing_command(name=name, cmd=cmd)
    return managed_simulators_list_from_stdout(stdout)


async def _list_managed_simulators(simulator_manager: str) -> list[Simulator]:
    list_cmd = _list_managed_simulators_command(simulator_manager=simulator_manager)
    return await _generic_managed_simulators_list_command(
        name="list managed simulators", cmd=list_cmd
    )


def normalize_os_version(os_version: str) -> Version:
    # os version should be in the format "iOS 17.2.0" or "iOS 17.2"
    return Version(os_version.split(" ")[1])


def choose_simulators(
    simulators: list[Simulator],
    simulator_type: SimulatorType,
    os_version: Optional[str],
    device: Optional[str],
) -> list[Simulator]:
    # If no device or os_version is specified, filter by simulator type
    if not device and not os_version:
        simulators = list(
            filter(
                lambda s: (s.is_type(simulator_type)),
                simulators,
            )
        )

    # Check if device is a device type identifier
    device_type_identifier: Optional[str] = None
    if device is not None and device.startswith("com.apple."):
        device_type_identifier = device
        device = None

    filtered_simulators = filter(
        lambda simulator: (
            (
                normalize_os_version(simulator.os_version).major
                == normalize_os_version(os_version).major
                and normalize_os_version(simulator.os_version).minor
                == normalize_os_version(os_version).minor
                if os_version
                else True
            )
            and (
                simulator.device_type_identifier == device_type_identifier
                if device_type_identifier is not None
                else True
            )
            and (simulator.name == device if device is not None else True)
        ),
        simulators,
    )

    return list(filtered_simulators)


async def _create_simulator(
    simulator_manager: str,
    simulator_type: SimulatorType,
    os_version: Optional[str] = None,
    device: Optional[str] = None,
) -> None:
    runtimes = await list_runtimes()
    spec = _select_simulator_spec(runtimes, simulator_type, os_version, device)
    spec_str = f"{spec.device},{spec.os_version}"
    create_cmd = _create_simulator_command(
        simulator_manager=simulator_manager, sim_spec=spec_str
    )
    await execute_generic_text_producing_command(
        name="create simulators", cmd=create_cmd
    )


async def _get_managed_simulators_create_if_needed(
    simulator_manager: str,
    simulator_type: SimulatorType,
    os_version: Optional[str] = None,
    device: Optional[str] = None,
) -> list[Simulator]:
    managed_simulators = await _get_managed_simulators(
        simulator_manager=simulator_manager,
        simulator_type=simulator_type,
        os_version=os_version,
        device=device,
    )
    if managed_simulators:
        return managed_simulators

    await _create_simulator(
        simulator_manager=simulator_manager,
        simulator_type=simulator_type,
        os_version=os_version,
        device=device,
    )
    managed_simulators = await _get_managed_simulators(
        simulator_manager=simulator_manager,
        simulator_type=simulator_type,
        os_version=os_version,
        device=device,
    )
    if managed_simulators:
        return managed_simulators

    raise RuntimeError(
        "Failed to create a simulator. Try to `sudo xcode-select -s <path_to_xcode>` and *open Xcode to install all required components*."
    )


async def _get_managed_simulators(
    simulator_manager: str,
    simulator_type: SimulatorType,
    os_version: Optional[str] = None,
    device: Optional[str] = None,
) -> list[Simulator]:
    managed_simulators = await _list_managed_simulators(
        simulator_manager=simulator_manager
    )
    return choose_simulators(managed_simulators, simulator_type, os_version, device)


def _select_simulator(
    only_booted: bool, all_simulators: list[Simulator]
) -> Optional[Simulator]:
    return next(
        filter(
            lambda s: s.state == SimulatorState.booted if only_booted else True,
            iter(all_simulators),
        ),
        None,
    )


def _select_simulator_with_preference(
    prefer_booted: bool, all_simulators: list[Simulator]
) -> Simulator:
    simulator = _select_simulator(
        only_booted=prefer_booted, all_simulators=all_simulators
    )
    if not simulator and prefer_booted:
        simulator = _select_simulator(only_booted=False, all_simulators=all_simulators)
    if not simulator:
        raise RuntimeError("Expected at least unbooted simulator entity to be selected")
    return simulator


async def prepare_simulator(
    simulator_manager: str,
    simulator_type: SimulatorType,
    os_version: Optional[str] = None,
    device: Optional[str] = None,
) -> SimulatorInfo:
    managed_simulators = await _get_managed_simulators_create_if_needed(
        simulator_manager=simulator_manager,
        simulator_type=simulator_type,
        os_version=os_version,
        device=device,
    )

    simulator = _select_simulator_with_preference(
        prefer_booted=simulator_type.booted(), all_simulators=managed_simulators
    )

    if simulator.state != SimulatorState.booted and simulator_type.booted():
        boot_cmd = _boot_simulator_command(
            simulator_manager=simulator_manager, udid=simulator.udid
        )
        await execute_generic_text_producing_command(
            name="boot simulator",
            cmd=boot_cmd,
            timeout=SIMULATOR_BOOT_TIMEOUT,
        )

    return SimulatorInfo(
        udid=simulator.udid,
        device_set_path=_device_set_path(),
    )
