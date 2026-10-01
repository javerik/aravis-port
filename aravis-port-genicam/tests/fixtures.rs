use aravis_port_genicam::{ChunkDataAccess, GenApiTree, MemoryRegisterAccess, RegisterAccess};

#[test]
fn literal_values_of_every_basic_type() {
    let xml = r#"<RegisterDescription>
      <Category Name="Root">
        <pFeature>Width</pFeature>
        <pFeature>Gain</pFeature>
        <pFeature>ReverseX</pFeature>
        <pFeature>PixelFormat</pFeature>
      </Category>
      <Integer Name="Width"><Value>64</Value></Integer>
      <Float Name="Gain"><Value>1.5</Value></Float>
      <Boolean Name="ReverseX"><Value>1</Value></Boolean>
      <Enumeration Name="PixelFormat">
        <EnumEntry Name="Mono8"><Value>1</Value></EnumEntry>
        <EnumEntry Name="Mono16"><Value>3</Value></EnumEntry>
        <Value>1</Value>
      </Enumeration>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(16);

    assert_eq!(tree.get_integer(&mut io, "Width").unwrap(), 64);
    assert_eq!(tree.get_float(&mut io, "Gain").unwrap(), 1.5);
    assert!(tree.get_boolean(&mut io, "ReverseX").unwrap());
    assert_eq!(tree.get_enum_symbolic(&mut io, "PixelFormat").unwrap(), "Mono8");
    assert_eq!(
        tree.category_children("Root").unwrap().len(),
        4
    );
}

#[test]
fn boolean_literals_true_false_are_accepted() {
    // Real vendor XML (confirmed on the live C5-2040-GigE camera) uses "true"/"false" text for
    // Boolean <Value> content, not just "1"/"0".
    let xml = r#"<RegisterDescription>
      <Boolean Name="A"><Value>true</Value></Boolean>
      <Boolean Name="B"><Value>false</Value></Boolean>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(4);
    assert!(tree.get_boolean(&mut io, "A").unwrap());
    assert!(!tree.get_boolean(&mut io, "B").unwrap());
}

#[test]
fn masked_bitfield_register_signed_and_unsigned() {
    // BigEndian MaskedIntReg LSB/MSB are given in wire/network bit order, which is the reverse
    // of the register's physical bit position — confirmed against every masked register in the
    // live C5-2040-GigE's actual XML (e.g. its GevSCPSPacketSize register declares
    // <LSB>31</LSB><MSB>16</MSB> to mean physical bits [15:0]). To target physical bits [4:7]
    // (the high nibble of the low byte) on this 4-byte (32-bit) register, the wire-order XML
    // values are LSB=27/MSB=24 (`31 - physical_bit`); physical bits [0:3] are wire LSB=31/MSB=28.
    let xml = r#"<RegisterDescription>
      <MaskedIntReg Name="UnsignedField">
        <Address>0x0</Address>
        <Length>4</Length>
        <LSB>27</LSB>
        <MSB>24</MSB>
        <Sign>Unsigned</Sign>
        <Endianess>BigEndian</Endianess>
      </MaskedIntReg>
      <MaskedIntReg Name="SignedField">
        <Address>0x0</Address>
        <Length>4</Length>
        <LSB>31</LSB>
        <MSB>28</MSB>
        <Sign>Signed</Sign>
        <Endianess>BigEndian</Endianess>
      </MaskedIntReg>
      <Integer Name="Unsigned"><pValue>UnsignedField</pValue></Integer>
      <Integer Name="Signed"><pValue>SignedField</pValue></Integer>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(16);
    io.write_memory(0, &0xAFu32.to_be_bytes()).unwrap();

    // Physical bits [4:7] = 0xA, physical bits [0:3] = 0xF == -1 in signed 4-bit two's complement.
    assert_eq!(tree.get_integer(&mut io, "Unsigned").unwrap(), 0xA);
    assert_eq!(tree.get_integer(&mut io, "Signed").unwrap(), -1);

    tree.set_integer(&mut io, "Unsigned", 0x5).unwrap();
    let raw = u32::from_be_bytes(io.bytes[0..4].try_into().unwrap());
    assert_eq!(raw, 0x5F); // high nibble replaced, low nibble (0xF) preserved
}

#[test]
fn multi_term_address_and_pindex_style_swiss_knife() {
    // Mirrors the shape of the real ArvGevSCPHostPortReg pattern: Address + pAddress (a
    // SwissKnife computing an offset) summed together.
    let xml = r#"<RegisterDescription>
      <Integer Name="Selector"><Value>2</Value></Integer>
      <IntSwissKnife Name="AddrCalc">
        <pVariable Name="SEL">Selector</pVariable>
        <Formula>SEL * 0x10</Formula>
      </IntSwissKnife>
      <IntReg Name="Reg">
        <Address>0x100</Address>
        <pAddress>AddrCalc</pAddress>
        <Length>4</Length>
        <Endianess>BigEndian</Endianess>
      </IntReg>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(0x200);
    // Address should resolve to 0x100 + 2*0x10 = 0x120.
    io.write_memory(0x120, &42u32.to_be_bytes()).unwrap();

    assert_eq!(tree.get_integer(&mut io, "Reg").unwrap(), 42);
}

#[test]
fn converter_with_distinct_formula_to_and_from() {
    let xml = r#"<RegisterDescription>
      <IntReg Name="RawReg">
        <Address>0x0</Address>
        <Length>4</Length>
        <Endianess>BigEndian</Endianess>
      </IntReg>
      <Integer Name="Raw"><pValue>RawReg</pValue></Integer>
      <IntConverter Name="Scaled">
        <pValue>Raw</pValue>
        <FormulaTo>FROM / 2</FormulaTo>
        <FormulaFrom>TO * 2</FormulaFrom>
      </IntConverter>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(16);
    io.write_memory(0, &10u32.to_be_bytes()).unwrap();

    // Reading: TO=raw(10), FormulaFrom = TO*2 = 20.
    assert_eq!(tree.get_integer(&mut io, "Scaled").unwrap(), 20);

    // Writing 50: FormulaTo = FROM/2 = 25, written into Raw.
    tree.set_integer(&mut io, "Scaled", 50).unwrap();
    assert_eq!(tree.get_integer(&mut io, "Raw").unwrap(), 25);
}

#[test]
fn cachable_register_skips_repeated_device_reads_until_invalidator_changes() {
    let xml = r#"<RegisterDescription>
      <Integer Name="Selector"><Value>0</Value></Integer>
      <IntReg Name="CachedReg">
        <Address>0x0</Address>
        <Length>4</Length>
        <Endianess>BigEndian</Endianess>
        <Cachable>WriteThrough</Cachable>
        <pInvalidator>Selector</pInvalidator>
      </IntReg>
      <Integer Name="Cached"><pValue>CachedReg</pValue></Integer>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(16);
    io.write_memory(0, &7u32.to_be_bytes()).unwrap();

    assert_eq!(tree.get_integer(&mut io, "Cached").unwrap(), 7);
    let reads_after_first = io.read_count;
    assert_eq!(tree.get_integer(&mut io, "Cached").unwrap(), 7);
    assert_eq!(io.read_count, reads_after_first, "second read should hit cache, not the device");

    // Selector is a plain literal-valued Integer (not writable) in this fixture, so directly
    // bump its epoch isn't possible via set_integer; instead verify caching survives when the
    // invalidator hasn't changed, and that disabling the cache always re-reads.
    tree.set_caching_enabled(false);
    io.write_memory(0, &99u32.to_be_bytes()).unwrap();
    assert_eq!(tree.get_integer(&mut io, "Cached").unwrap(), 99);
}

#[test]
fn writable_invalidator_forces_cache_refresh() {
    let xml = r#"<RegisterDescription>
      <IntReg Name="SelectorReg">
        <Address>0x10</Address>
        <Length>4</Length>
        <Endianess>BigEndian</Endianess>
      </IntReg>
      <Integer Name="Selector"><pValue>SelectorReg</pValue></Integer>
      <IntReg Name="BankedReg">
        <Address>0x0</Address>
        <Length>4</Length>
        <Endianess>BigEndian</Endianess>
        <Cachable>WriteThrough</Cachable>
        <pInvalidator>Selector</pInvalidator>
      </IntReg>
      <Integer Name="Banked"><pValue>BankedReg</pValue></Integer>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(32);
    io.write_memory(0, &1u32.to_be_bytes()).unwrap();

    assert_eq!(tree.get_integer(&mut io, "Banked").unwrap(), 1);
    let reads_before = io.read_count;
    assert_eq!(tree.get_integer(&mut io, "Banked").unwrap(), 1);
    assert_eq!(io.read_count, reads_before, "cached, no new device read");

    // Changing the selector (an invalidator of BankedReg) must force a fresh read next time,
    // even though BankedReg's own bytes at address 0 haven't changed.
    tree.set_integer(&mut io, "Selector", 5).unwrap();
    io.write_memory(0, &2u32.to_be_bytes()).unwrap();
    assert_eq!(tree.get_integer(&mut io, "Banked").unwrap(), 2);
}

#[test]
fn dangling_reference_is_reported_not_panicking() {
    let xml = r#"<RegisterDescription>
      <Integer Name="Broken"><pValue>DoesNotExist</pValue></Integer>
    </RegisterDescription>"#;
    assert!(GenApiTree::parse(xml).is_err());
}

#[test]
fn struct_entries_are_bit_fields_of_their_struct_reg() {
    // Shaped like the live C6-2040-GigE's `Scan3dCapabilities` StructReg: the register elements
    // live on the StructReg and each StructEntry adds only its name and bit range (BigEndian
    // wire bit order, as for MaskedIntReg). Both entries are cached, so writing one must
    // invalidate the other's cached bytes, or the second read-modify-write would clobber the
    // first field.
    let xml = r#"<RegisterDescription>
      <Integer Name="FilterSize"><pValue>FilterSizeReg</pValue></Integer>
      <StructReg Comment="Capabilities">
        <Address>0x0</Address>
        <Length>4</Length>
        <AccessMode>RW</AccessMode>
        <pPort>Device</pPort>
        <Cachable>WriteThrough</Cachable>
        <Endianess>BigEndian</Endianess>
        <StructEntry Name="IsFirAvailableReg"><Bit>31</Bit></StructEntry>
        <StructEntry Name="FilterSizeReg"><LSB>11</LSB><MSB>8</MSB></StructEntry>
      </StructReg>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(4);

    assert_eq!(tree.get_integer(&mut io, "IsFirAvailableReg").unwrap(), 0);
    tree.set_integer(&mut io, "FilterSize", 5).unwrap();
    tree.set_integer(&mut io, "IsFirAvailableReg", 1).unwrap();

    assert_eq!(u32::from_be_bytes(io.bytes[..4].try_into().unwrap()), (5 << 20) | 1);
    assert_eq!(tree.get_integer(&mut io, "FilterSize").unwrap(), 5);
    assert_eq!(tree.get_integer(&mut io, "IsFirAvailableReg").unwrap(), 1);
}

#[test]
fn string_nodes_are_literal_or_delegate_to_a_string_reg() {
    // Shaped like the live C6-2040-GigE's `EventLogMessageText` (pValue -> StringReg) and
    // `ValueArrayCandidates` (literal Value).
    let xml = r#"<RegisterDescription>
      <Category Name="Root"><pFeature>MessageText</pFeature></Category>
      <String Name="Candidates"><Value>A,B</Value></String>
      <String Name="MessageText"><pValue>MessageText_Reg</pValue></String>
      <StringReg Name="MessageText_Reg">
        <Address>0x0</Address>
        <Length>8</Length>
        <AccessMode>RW</AccessMode>
      </StringReg>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(8);

    assert_eq!(tree.node_kind(tree.node_id("MessageText").unwrap()), "String");
    assert_eq!(tree.get_string(&mut io, "Candidates").unwrap(), "A,B");
    assert!(tree.set_string(&mut io, "Candidates", "C").is_err());

    tree.set_string(&mut io, "MessageText", "hello").unwrap();
    assert_eq!(&io.bytes[..6], b"hello\0");
    assert_eq!(tree.get_string(&mut io, "MessageText").unwrap(), "hello");
}

#[test]
fn register_description_attributes_are_exposed() {
    let tree = GenApiTree::parse(
        r#"<RegisterDescription ModelName="C6_X_GigE" VendorName="AT_Automation_Technology_GmbH"
             SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
           </RegisterDescription>"#,
    )
    .unwrap();
    let description = tree.register_description();
    assert_eq!(description.vendor_name, "AT_Automation_Technology_GmbH");
    assert_eq!(description.model_name, "C6_X_GigE");
    assert_eq!(description.schema_version, (1, 1, 0));

    // Missing schema attributes default to 0.0.0, as in Aravis.
    let bare = GenApiTree::parse("<RegisterDescription></RegisterDescription>").unwrap();
    assert_eq!(bare.register_description().schema_version, (0, 0, 0));
}

/// A GigE Vision chunk payload: `[image][IMAGE][len]` followed by each `(id, data)` chunk with
/// its big-endian `[id][len]` trailer.
fn chunk_payload(image: &[u8], chunks: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut payload = image.to_vec();
    payload.extend_from_slice(&0xa6a6_a6a6u32.to_be_bytes());
    payload.extend_from_slice(&(image.len() as u32).to_be_bytes());
    for (id, data) in chunks {
        payload.extend_from_slice(data);
        payload.extend_from_slice(&id.to_be_bytes());
        payload.extend_from_slice(&(data.len() as u32).to_be_bytes());
    }
    payload
}

#[test]
fn chunk_features_read_from_buffer_chunks_with_a_scan_line_index() {
    // Shaped like the live C6-2040-GigE's chunk features: little-endian registers behind a
    // chunk port, 64-byte scan-line records selected by `pIndex Offset="64"`, and availability
    // gated on a device register.
    let xml = r#"<RegisterDescription>
      <Port Name="Device"/>
      <Port Name="CameraChunkPortFrame"><ChunkID>44443333</ChunkID></Port>
      <Integer Name="ScanLine"><Value>1</Value></Integer>
      <IntReg Name="ModeReg">
        <Address>0x0</Address><Length>4</Length><pPort>Device</pPort>
      </IntReg>
      <Integer Name="ChunkFrameID">
        <pIsAvailable>ModeReg</pIsAvailable>
        <pValue>ChunkFrameIDValue</pValue>
      </Integer>
      <IntReg Name="ChunkFrameIDValue">
        <Address>8</Address>
        <pIndex Offset="64">ScanLine</pIndex>
        <Length>8</Length>
        <AccessMode>RO</AccessMode>
        <pPort>CameraChunkPortFrame</pPort>
        <Endianess>LittleEndian</Endianess>
      </IntReg>
      <IntReg Name="ChunkLineStatusAllValue">
        <Address>40</Address>
        <pIndex Offset="64">ScanLine</pIndex>
        <Length>2</Length>
        <AccessMode>RO</AccessMode>
        <pPort>CameraChunkPortFrame</pPort>
        <Endianess>LittleEndian</Endianess>
      </IntReg>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut device = MemoryRegisterAccess::new(4);

    let mut records = vec![0u8; 128];
    records[64 + 8..64 + 16].copy_from_slice(&351_241u64.to_le_bytes());
    records[64 + 40..64 + 42].copy_from_slice(&28u16.to_le_bytes());
    let payload = chunk_payload(b"pixels", &[(0x4444_2222, vec![0; 64]), (0x4444_3333, records)]);

    let mut chunks = ChunkDataAccess::new(&mut device, &payload).unwrap();
    assert_eq!(tree.get_integer(&mut chunks, "ChunkFrameID").unwrap(), 351_241);
    assert_eq!(tree.get_integer(&mut chunks, "ChunkLineStatusAllValue").unwrap(), 28);
    assert!(tree.set_integer(&mut chunks, "ChunkFrameID", 1).is_err(), "chunk data is read-only");

    // Availability follows the device register, not the chunk.
    assert!(!tree.is_available(&mut device, "ChunkFrameID").unwrap());
    device.write_memory(0, &1u32.to_be_bytes()).unwrap();
    assert!(tree.is_available(&mut device, "ChunkFrameID").unwrap());

    // Without a buffer, or with a buffer lacking the chunk, the read fails instead of reading
    // device memory or panicking.
    assert!(tree.get_integer(&mut device, "ChunkFrameID").is_err());
    let without_frame_chunk = chunk_payload(b"pixels", &[(0x4444_2222, vec![0; 64])]);
    let mut chunks = ChunkDataAccess::new(&mut device, &without_frame_chunk).unwrap();
    assert!(tree.get_integer(&mut chunks, "ChunkFrameID").is_err());
    // A chunk too short for the selected record is an error, not an out-of-bounds panic.
    let short = chunk_payload(b"pixels", &[(0x4444_3333, vec![0; 16])]);
    let mut chunks = ChunkDataAccess::new(&mut device, &short).unwrap();
    assert!(tree.get_integer(&mut chunks, "ChunkFrameID").is_err());
}

#[test]
fn malformed_chunk_id_is_an_error_not_a_panic() {
    let xml = r#"<RegisterDescription>
      <Port Name="ChunkPort"><ChunkID>not-hex</ChunkID></Port>
    </RegisterDescription>"#;
    assert!(GenApiTree::parse(xml).is_err());
}

#[test]
fn invalidate_cache_forces_a_fresh_device_read() {
    // A cached register whose device value changes behind the tree's back (a raw register
    // write it has no invalidator for) stays stale until the cache is dropped.
    let xml = r#"<RegisterDescription>
      <IntReg Name="PayloadSizeReg">
        <Address>0x0</Address><Length>4</Length><Cachable>WriteAround</Cachable>
      </IntReg>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(4);
    io.write_memory(0, &100u32.to_be_bytes()).unwrap();
    assert_eq!(tree.get_integer(&mut io, "PayloadSizeReg").unwrap(), 100);

    io.write_memory(0, &200u32.to_be_bytes()).unwrap();
    assert_eq!(tree.get_integer(&mut io, "PayloadSizeReg").unwrap(), 100, "served from cache");
    tree.invalidate_cache();
    assert_eq!(tree.get_integer(&mut io, "PayloadSizeReg").unwrap(), 200);
}

#[test]
fn a_cached_selector_indexed_register_is_reread_when_the_selector_moves_it() {
    // `GainReg`'s address follows `GainSelector` through `pIndex`, and like many real documents
    // it declares no `pInvalidator` for it. The cached bytes belong to the old address, so they
    // must not be served once the selector points elsewhere.
    let xml = r#"<RegisterDescription>
      <Enumeration Name="GainSelector">
        <EnumEntry Name="All"><Value>0</Value></EnumEntry>
        <EnumEntry Name="Red"><Value>1</Value></EnumEntry>
        <pValue>GainSelectorReg</pValue>
        <pSelected>Gain</pSelected>
      </Enumeration>
      <IntReg Name="GainSelectorReg">
        <Address>0x0</Address><Length>4</Length><Endianess>BigEndian</Endianess>
        <Cachable>WriteThrough</Cachable>
      </IntReg>
      <IntReg Name="GainReg">
        <Address>0x10</Address>
        <pIndex Offset="4">GainSelector</pIndex>
        <Length>4</Length><Endianess>BigEndian</Endianess>
        <Cachable>WriteThrough</Cachable>
      </IntReg>
      <Integer Name="Gain"><pValue>GainReg</pValue></Integer>
    </RegisterDescription>"#;
    let tree = GenApiTree::parse(xml).unwrap();
    let mut io = MemoryRegisterAccess::new(32);
    io.write_memory(0x10, &7u32.to_be_bytes()).unwrap();
    io.write_memory(0x14, &9u32.to_be_bytes()).unwrap();

    assert_eq!(tree.get_integer(&mut io, "Gain").unwrap(), 7);
    tree.set_enum_symbolic(&mut io, "GainSelector", "Red").unwrap();
    assert_eq!(tree.get_integer(&mut io, "Gain").unwrap(), 9, "read from the Red gain's address");
    tree.set_enum_symbolic(&mut io, "GainSelector", "All").unwrap();
    assert_eq!(tree.get_integer(&mut io, "Gain").unwrap(), 7);

    // An unchanged selector still hits the cache.
    let reads_before = io.read_count;
    assert_eq!(tree.get_integer(&mut io, "Gain").unwrap(), 7);
    assert_eq!(io.read_count, reads_before, "cached, no new device read");
}
