use aravis_port_genicam::{GenApiTree, MemoryRegisterAccess, RegisterAccess};

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
