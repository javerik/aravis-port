//! A small, hand-authored GenICam XML document exercising the node types the live
//! AT-Automation Technology C5-2040-GigE camera actually uses (`Category`, `Integer`,
//! `IntReg`, `Enumeration`+`EnumEntry`, `Float`, `Command`), used to unblock
//! `aravis-port-device` integration tests ahead of the full GenICam engine (Phase 3). The
//! frame-rate, trigger and reverse features mirror the C6-2040-GigE's bounds/lock shapes
//! (`pMax`, `Unit`, `pIsLocked`) for `GenApiTree::feature_info`; the fake only stores them.

use crate::registers::feature;

pub fn minimal_genicam_xml() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<RegisterDescription xmlns="http://www.genicam.org/GenApi/Version_1_1" ModelName="FakeCamera" VendorName="aravis-port" SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <Category Name="Root">
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
  </Category>

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
    <Max>4096</Max>
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
    <Max>4096</Max>
  </Integer>

  <IntReg Name="PixelFormatReg">
    <Address>0x{pixel_format_addr:x}</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Sign>Unsigned</Sign>
    <Endianess>BigEndian</Endianess>
  </IntReg>
  <Enumeration Name="PixelFormat">
    <EnumEntry Name="Mono8"><Value>1</Value></EnumEntry>
    <EnumEntry Name="Mono10"><Value>2</Value></EnumEntry>
    <EnumEntry Name="Mono16"><Value>3</Value></EnumEntry>
    <pValue>PixelFormatReg</pValue>
  </Enumeration>

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
        ] {
            assert!(xml.contains(&format!("Name=\"{name}\"")), "missing feature {name}");
        }
    }
}
