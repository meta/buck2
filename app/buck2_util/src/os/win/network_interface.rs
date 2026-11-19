/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_ETHERNET_CSMACD;
use windows_sys::Win32::NetworkManagement::IpHelper::IF_TYPE_IEEE80211;
use windows_sys::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2;
use windows_sys::Win32::NetworkManagement::Ndis::MediaConnectStateConnected;

#[derive(Copy, Clone, Debug)]
pub enum NetworkKind {
    WiFi,
    Ethernet,
    Unknown,
}

/// Representation of a Windows network interface.
pub struct NetworkInterface {
    mib_entry: MIB_IF_ROW2,
}

impl NetworkInterface {
    pub fn new(mib_entry: MIB_IF_ROW2) -> Self {
        Self { mib_entry }
    }

    pub fn name(&self) -> String {
        let name_len = self
            .mib_entry
            .Alias
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(self.mib_entry.Alias.len());
        String::from_utf16(&self.mib_entry.Alias[..name_len])
            .unwrap_or_else(|_| String::from("<Unknown>"))
    }

    pub fn hardware_interface(&self) -> bool {
        // Bit 0 of InterfaceAndOperStatusFlags is HardwareInterface.
        // windows-sys doesn't generate bitfield accessors, so we read it directly.
        self.mib_entry.InterfaceAndOperStatusFlags._bitfield & 1 != 0
    }

    pub fn bytes_sent(&self) -> u64 {
        self.mib_entry.OutOctets
    }

    pub fn bytes_recv(&self) -> u64 {
        self.mib_entry.InOctets
    }

    pub fn network_kind(&self) -> NetworkKind {
        match self.mib_entry.Type {
            IF_TYPE_ETHERNET_CSMACD => NetworkKind::Ethernet,
            IF_TYPE_IEEE80211 => NetworkKind::WiFi,
            _ => NetworkKind::Unknown,
        }
    }

    pub fn description(&self) -> String {
        let desc_len = self
            .mib_entry
            .Description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(self.mib_entry.Description.len());
        String::from_utf16(&self.mib_entry.Description[..desc_len])
            .unwrap_or_else(|_| String::from("<Unknown>"))
    }

    pub fn is_connected(&self) -> bool {
        self.mib_entry.MediaConnectState == MediaConnectStateConnected
    }
}
