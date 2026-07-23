//! A small, hand-authored GenICam XML document exercising the node types the live
//! AT-Automation Technology C5-2040-GigE camera actually uses (`Category`, `Integer`,
//! `IntReg`, `Enumeration`+`EnumEntry`, `Float`, `Command`), used to unblock
//! `aravis-port-device` integration tests ahead of the full GenICam engine (Phase 3).

use crate::registers::feature;

pub fn minimal_genicam_xml() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<RegisterDescription xmlns="http://www.genicam.org/GenApi/Version_1_1" ModelName="FakeCamera" VendorName="aravis-port">
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
  </Category>

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
    <pValue>ExposureTimeReg</pValue>
    <Min>1</Min>
    <Max>1000000</Max>
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

  <IntSwissKnife Name="PayloadSize">
    <pVariable Name="W">Width</pVariable>
    <pVariable Name="H">Height</pVariable>
    <Formula>W * H</Formula>
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
        ] {
            assert!(xml.contains(&format!("Name=\"{name}\"")), "missing feature {name}");
        }
    }
}
