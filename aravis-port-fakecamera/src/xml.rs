//! A small, hand-authored GenICam XML document exercising the node types the live
//! AT-Automation Technology C5-2040-GigE camera actually uses (`Category`, `Integer`,
//! `IntReg`, `Enumeration`+`EnumEntry`, `Float`, `Command`), used to unblock
//! `aravis-port-device` integration tests ahead of the full GenICam engine (Phase 3). The
//! frame-rate, trigger and reverse features mirror the C6-2040-GigE's bounds/lock shapes
//! (`pMax`, `Unit`, `pIsLocked`) for `GenApiTree::feature_info`; the fake only stores them.
//! The region (`OffsetX`/`OffsetY` against `WidthMax`/`HeightMax`) follows the SFNC shape, with
//! each size's max shrinking by its offset and the other way round. The transport features
//! (`GevSCPSPacketSize` and its flag bits) map the standard bootstrap register, as camera XMLs do.
//! `DeviceScanType` switches between area scan and a C6-like `Linescan3D`, whose only
//! `PixelFormat` is `Coord3D_C16`. The entries say so through `pIsAvailable`, and the device
//! refuses the others (see `gvcp_server`). The `Scan3dCoordinate*` features report a fixed
//! calibration per coordinate.

use crate::registers::feature;

pub fn minimal_genicam_xml() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<RegisterDescription xmlns="http://www.genicam.org/GenApi/Version_1_1" ModelName="FakeCamera" VendorName="aravis-port" SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Category Name="Root">
    <pFeature>DeviceScanType</pFeature>
    <pFeature>Width</pFeature>
    <pFeature>Height</pFeature>
    <pFeature>PixelFormat</pFeature>
    <pFeature>ExposureTime</pFeature>
    <pFeature>Gain</pFeature>
    <pFeature>AcquisitionStart</pFeature>
    <pFeature>AcquisitionStop</pFeature>
    <pFeature>ChunkModeActive</pFeature>
    <pFeature>PayloadSize</pFeature>
    <pFeature>AcquisitionFrameRateEnable</pFeature>
    <pFeature>AcquisitionFrameRate</pFeature>
    <pFeature>TriggerSoftware</pFeature>
    <pFeature>ReverseX</pFeature>
    <pFeature>OffsetX</pFeature>
    <pFeature>OffsetY</pFeature>
    <pFeature>WidthMax</pFeature>
    <pFeature>HeightMax</pFeature>
    <pFeature>Scan3dControl</pFeature>
    <pFeature>TransportLayerControl</pFeature>
  </Category>

  <Category Name="Scan3dControl">
    <pFeature>Scan3dCoordinateSelector</pFeature>
    <pFeature>Scan3dCoordinateScale</pFeature>
    <pFeature>Scan3dCoordinateOffset</pFeature>
  </Category>

  <Category Name="TransportLayerControl">
    <pFeature>GevSCPSPacketSize</pFeature>
    <pFeature>GevSCPSDoNotFragment</pFeature>
    <pFeature>GevSCPSFireTestPacket</pFeature>
  </Category>

  <MaskedIntReg Name="GevSCPSPacketSizeReg">
    <Address>0x{packet_size_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <LSB>31</LSB>
    <MSB>16</MSB>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </MaskedIntReg>
  <Integer Name="GevSCPSPacketSize">
    <pValue>GevSCPSPacketSizeReg</pValue>
    <Min>576</Min>
    <Max>9000</Max>
    <Inc>4</Inc>
    <Unit>B</Unit>
  </Integer>
  <MaskedIntReg Name="GevSCPSDoNotFragmentReg">
    <Address>0x{packet_size_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Bit>1</Bit>
    <Endianess>BigEndian</Endianess>
  </MaskedIntReg>
  <Boolean Name="GevSCPSDoNotFragment">
    <pValue>GevSCPSDoNotFragmentReg</pValue>
  </Boolean>
  <MaskedIntReg Name="GevSCPSFireTestPacketReg">
    <Address>0x{packet_size_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Bit>0</Bit>
    <Endianess>BigEndian</Endianess>
  </MaskedIntReg>
  <Command Name="GevSCPSFireTestPacket">
    <pValue>GevSCPSFireTestPacketReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>

  <Integer Name="TLParamsLocked">
    <ImposedAccessMode>RW</ImposedAccessMode>
    <Value>0</Value>
  </Integer>

  <IntReg Name="WidthReg">
    <Address>0x{width_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="Width">
    <pValue>WidthReg</pValue>
    <Min>1</Min>
    <pMax>WidthAvailable</pMax>
  </Integer>

  <IntReg Name="HeightReg">
    <Address>0x{height_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="Height">
    <pValue>HeightReg</pValue>
    <Min>1</Min>
    <pMax>HeightAvailable</pMax>
  </Integer>

  <Integer Name="WidthMax">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <Value>4096</Value>
  </Integer>
  <Integer Name="HeightMax">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <Value>4096</Value>
  </Integer>
  <IntSwissKnife Name="WidthAvailable">
    <pVariable Name="M">WidthMax</pVariable>
    <pVariable Name="O">OffsetXReg</pVariable>
    <Formula>M - O</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="HeightAvailable">
    <pVariable Name="M">HeightMax</pVariable>
    <pVariable Name="O">OffsetYReg</pVariable>
    <Formula>M - O</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="OffsetXAvailable">
    <pVariable Name="M">WidthMax</pVariable>
    <pVariable Name="W">WidthReg</pVariable>
    <Formula>M - W</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="OffsetYAvailable">
    <pVariable Name="M">HeightMax</pVariable>
    <pVariable Name="H">HeightReg</pVariable>
    <Formula>M - H</Formula>
  </IntSwissKnife>

  <IntReg Name="OffsetXReg">
    <Address>0x{offset_x_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="OffsetX">
    <pValue>OffsetXReg</pValue>
    <Min>0</Min>
    <pMax>OffsetXAvailable</pMax>
    <Inc>2</Inc>
  </Integer>
  <IntReg Name="OffsetYReg">
    <Address>0x{offset_y_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="OffsetY">
    <pValue>OffsetYReg</pValue>
    <Min>0</Min>
    <pMax>OffsetYAvailable</pMax>
    <Inc>2</Inc>
  </Integer>

  <IntReg Name="PixelFormatReg">
    <Address>0x{pixel_format_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Enumeration Name="PixelFormat">
    <EnumEntry Name="Mono8"><pIsAvailable>IsAreascan</pIsAvailable><Value>1</Value></EnumEntry>
    <EnumEntry Name="Mono10"><pIsAvailable>IsAreascan</pIsAvailable><Value>2</Value></EnumEntry>
    <EnumEntry Name="Mono16"><pIsAvailable>IsAreascan</pIsAvailable><Value>3</Value></EnumEntry>
    <EnumEntry Name="Coord3D_C16"><pIsAvailable>IsLinescan3D</pIsAvailable><Value>4</Value></EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>

  <IntReg Name="DeviceScanTypeReg">
    <Address>0x{scan_type_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Enumeration Name="DeviceScanType">
    <pIsLocked>TLParamsLocked</pIsLocked>
    <EnumEntry Name="Areascan"><Value>0</Value></EnumEntry>
    <EnumEntry Name="Linescan3D"><Value>1</Value></EnumEntry>
    <pValue>DeviceScanTypeReg</pValue>
  </Enumeration>
  <IntSwissKnife Name="IsAreascan">
    <pVariable Name="S">DeviceScanTypeReg</pVariable>
    <Formula>S = 0</Formula>
  </IntSwissKnife>
  <IntSwissKnife Name="IsLinescan3D">
    <pVariable Name="S">DeviceScanTypeReg</pVariable>
    <Formula>S = 1</Formula>
  </IntSwissKnife>

  <IntReg Name="Scan3dCoordinateSelectorReg">
    <Address>0x{scan3d_selector_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Enumeration Name="Scan3dCoordinateSelector">
    <pSelected>Scan3dCoordinateScale</pSelected>
    <pSelected>Scan3dCoordinateOffset</pSelected>
    <EnumEntry Name="CoordinateA"><Value>0</Value></EnumEntry>
    <EnumEntry Name="CoordinateB"><Value>1</Value></EnumEntry>
    <EnumEntry Name="CoordinateC"><Value>2</Value></EnumEntry>
    <pValue>Scan3dCoordinateSelectorReg</pValue>
  </Enumeration>
  <SwissKnife Name="Scan3dCoordinateScaleValue">
    <pVariable Name="S">Scan3dCoordinateSelectorReg</pVariable>
    <Formula>S = 0 ? 0.05 : (S = 1 ? 0.1 : 0.001)</Formula>
  </SwissKnife>
  <Float Name="Scan3dCoordinateScale">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <pValue>Scan3dCoordinateScaleValue</pValue>
    <Unit>mm</Unit>
  </Float>
  <Float Name="Scan3dCoordinateOffset">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <Value>0</Value>
    <Unit>mm</Unit>
  </Float>

  <IntReg Name="ExposureTimeReg">
    <Address>0x{exposure_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Float Name="ExposureTime">
    <pIsLocked>TLParamsLocked</pIsLocked>
    <pValue>ExposureTimeReg</pValue>
    <Min>1</Min>
    <Max>1000000</Max>
    <Inc>1</Inc>
    <Unit>us</Unit>
    <Representation>Linear</Representation>
  </Float>

  <IntReg Name="GainReg">
    <Address>0x{gain_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="Gain">
    <pValue>GainReg</pValue>
    <Min>0</Min>
    <Max>100</Max>
  </Integer>

  <IntReg Name="AcquisitionActiveReg">
    <Address>0x{acquisition_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Command Name="AcquisitionStart">
    <pValue>AcquisitionActiveReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>
  <Command Name="AcquisitionStop">
    <pValue>AcquisitionActiveReg</pValue>
    <CommandValue>0</CommandValue>
  </Command>

  <IntReg Name="ChunkModeActiveReg">
    <Address>0x{chunk_mode_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Boolean Name="ChunkModeActive">
    <pValue>ChunkModeActiveReg</pValue>
  </Boolean>

  <Port Name="ChunkPortFrameID">
    <ChunkID>{chunk_id_frame_id:x}</ChunkID>
  </Port>
  <IntReg Name="ChunkFrameIDReg">
    <Address>0</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
    <pPort>ChunkPortFrameID</pPort>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Integer Name="ChunkFrameID">
    <pIsAvailable>ChunkModeActiveReg</pIsAvailable>
    <pValue>ChunkFrameIDReg</pValue>
  </Integer>

  <IntReg Name="AcquisitionFrameRateEnableReg">
    <Address>0x{frame_rate_enable_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Boolean Name="AcquisitionFrameRateEnable">
    <pValue>AcquisitionFrameRateEnableReg</pValue>
  </Boolean>
  <IntSwissKnife Name="isAcquisitionFrameRateDisabledOrTLParamsLocked">
    <pVariable Name="E">AcquisitionFrameRateEnableReg</pVariable>
    <pVariable Name="L">TLParamsLocked</pVariable>
    <Formula>(E = 0) || L</Formula>
  </IntSwissKnife>
  <Integer Name="SensorRateMax">
    <ImposedAccessMode>RO</ImposedAccessMode>
    <Value>500</Value>
  </Integer>
  <FloatReg Name="AcquisitionFrameRateReg">
    <Address>0x{frame_rate_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Endianess>BigEndian</Endianess>
  </FloatReg>
  <Float Name="AcquisitionFrameRate">
    <pIsLocked>isAcquisitionFrameRateDisabledOrTLParamsLocked</pIsLocked>
    <pValue>AcquisitionFrameRateReg</pValue>
    <Min>1</Min>
    <pMax>SensorRateMax</pMax>
    <Unit>Hz</Unit>
  </Float>

  <IntReg Name="TriggerSoftwareReg">
    <Address>0x{trigger_software_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Command Name="TriggerSoftware">
    <pValue>TriggerSoftwareReg</pValue>
    <CommandValue>1</CommandValue>
  </Command>

  <IntReg Name="ReverseXReg">
    <Address>0x{reverse_x_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Boolean Name="ReverseX">
    <pValue>ReverseXReg</pValue>
  </Boolean>

  <IntSwissKnife Name="PayloadSize">
    <pVariable Name="W">Width</pVariable>
    <pVariable Name="H">Height</pVariable>
    <pVariable Name="C">ChunkModeActiveReg</pVariable>
    <pVariable Name="P">PixelFormatReg</pVariable>
    <Formula>W * H * (P = 1 ? 1 : 2) + (C ? {chunk_extra_bytes} : 0)</Formula>
  </IntSwissKnife>
</RegisterDescription>
"#,
        packet_size_addr = aravis_port_core::bootstrap::offset::STREAM_CHANNEL_0_PACKET_SIZE,
        width_addr = feature::WIDTH,
        height_addr = feature::HEIGHT,
        pixel_format_addr = feature::PIXEL_FORMAT,
        exposure_addr = feature::EXPOSURE_TIME_US,
        gain_addr = feature::GAIN_RAW,
        acquisition_addr = feature::ACQUISITION_ACTIVE,
        chunk_mode_addr = feature::CHUNK_MODE_ACTIVE,
        frame_rate_enable_addr = feature::FRAME_RATE_ENABLE,
        frame_rate_addr = feature::FRAME_RATE,
        trigger_software_addr = feature::TRIGGER_SOFTWARE,
        reverse_x_addr = feature::REVERSE_X,
        offset_x_addr = feature::OFFSET_X,
        offset_y_addr = feature::OFFSET_Y,
        scan_type_addr = feature::DEVICE_SCAN_TYPE,
        scan3d_selector_addr = feature::SCAN3D_COORDINATE_SELECTOR,
        chunk_id_frame_id = crate::gvsp_server::CHUNK_ID_FRAME_ID,
        chunk_extra_bytes = crate::gvsp_server::CHUNK_MODE_EXTRA_BYTES,
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_is_well_formed_enough_to_contain_expected_features() {
        let xml = String::from_utf8(minimal_genicam_xml()).unwrap();
        for name in [
            "Width",
            "Height",
            "PixelFormat",
            "ExposureTime",
            "Gain",
            "AcquisitionStart",
            "AcquisitionStop",
            "ChunkModeActive",
            "PayloadSize",
            "AcquisitionFrameRateEnable",
            "AcquisitionFrameRate",
            "TriggerSoftware",
            "ReverseX",
            "OffsetX",
            "OffsetY",
            "WidthMax",
            "HeightMax",
            "GevSCPSPacketSize",
            "GevSCPSDoNotFragment",
            "GevSCPSFireTestPacket",
            "DeviceScanType",
            "Coord3D_C16",
            "Scan3dCoordinateSelector",
            "Scan3dCoordinateScale",
            "Scan3dCoordinateOffset",
        ] {
            assert!(xml.contains(&format!("Name=\"{name}\"")), "missing feature {name}");
        }
    }
}
