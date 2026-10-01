use crate::formula::Formula;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    Big,
    Little,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Signed,
    Unsigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cachable {
    NoCache,
    WriteThrough,
    WriteAround,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressTerm {
    Literal(u64),
    PAddress(NodeId),
    /// `<pIndex>`: adds `offset * value(index)` bytes.
    Index { index: NodeId, offset: IndexOffset },
}

/// The per-step byte stride of a `<pIndex>` term: its `Offset`/`pOffset` attribute, or the
/// register's own `Length` when it has neither (as in Aravis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOffset {
    RegisterLength,
    Literal(u64),
    PNode(NodeId),
}

/// The register-level shape of an `IntReg`/`MaskedIntReg`/`FloatReg`/`StringReg` node: address
/// resolution, byte layout, optional bit mask, caching policy, and invalidators.
#[derive(Debug, Clone)]
pub struct RegisterAccessSpec {
    pub address_terms: Vec<AddressTerm>,
    pub length: u32,
    pub endianness: Endianness,
    /// `(lsb, msb)` inclusive bit range, if this is a masked (bitfield) register.
    pub bit_mask: Option<(u8, u8)>,
    pub sign: Sign,
    pub cachable: Cachable,
    pub invalidators: Vec<NodeId>,
    pub writable: bool,
    /// `Some(id)` when the register's `pPort` is a chunk port (a `<Port>` with a `<ChunkID>`):
    /// its bytes come from chunk `id` of an acquired buffer rather than from device memory.
    pub chunk_id: Option<u32>,
}

impl Default for RegisterAccessSpec {
    fn default() -> Self {
        Self {
            address_terms: Vec::new(),
            length: 4,
            endianness: Endianness::Big,
            bit_mask: None,
            sign: Sign::Unsigned,
            cachable: Cachable::NoCache,
            invalidators: Vec::new(),
            writable: true,
            chunk_id: None,
        }
    }
}

/// Where a front-end feature node (`Integer`/`Float`/`Boolean`/`Enumeration`) gets its value
/// from.
#[derive(Debug, Clone)]
pub enum ValueSource {
    LiteralInt(i64),
    LiteralFloat(f64),
    /// `pValue`: delegate entirely to another node (an `IntReg`, `SwissKnife`, `Converter`, or
    /// another plain feature node).
    PValue(NodeId),
    /// `<pIndex>` plus `<ValueIndexed Index>`/`<pValueIndexed Index>` and
    /// `<ValueDefault>`/`<pValueDefault>`: the value is a lookup table keyed by another node's
    /// value. Read-only. Confirmed necessary against the live C6-2040-GigE, whose standard `Gain`
    /// maps the vendor `SensorGainReg` index to a gain factor this way.
    Indexed {
        index: NodeId,
        entries: Vec<(i64, ValueSource)>,
        default: Box<ValueSource>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct CategoryNode {
    pub children: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct NumericNode {
    pub value: ValueSource,
    /// `<Min>`/`<pMin>`, `<Max>`/`<pMax>` and `<Inc>`/`<pInc>`; a pointer wins over a literal.
    pub min: Option<ValueSource>,
    pub max: Option<ValueSource>,
    pub inc: Option<ValueSource>,
    /// `<Unit>` text (Float only in the standard, but read for Integer too).
    pub unit: Option<String>,
    /// `<Representation>` text (`Linear`, `Logarithmic`, `HexNumber`, `IPV4Address`, ...).
    pub representation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BooleanNode {
    pub value: ValueSource,
}

#[derive(Debug, Clone)]
pub struct EnumEntryNode {
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct EnumerationNode {
    pub entries: Vec<(String, NodeId)>,
    pub value: ValueSource,
}

/// Where a `<String>` node gets its text from: a literal `<Value>`, or `<pValue>` delegating to
/// another string node (typically a `StringReg`).
#[derive(Debug, Clone)]
pub enum StringSource {
    Literal(String),
    PValue(NodeId),
}

#[derive(Debug, Clone)]
pub struct CommandNode {
    pub value: ValueSource,
    pub command_value: i64,
}

/// Shared shape of `SwissKnife`/`IntSwissKnife`: a read-only formula over `pVariable`-resolved
/// inputs.
#[derive(Debug, Clone)]
pub struct SwissKnifeNode {
    pub formula: Formula,
    pub variables: Vec<(String, NodeId)>,
    /// `true` for `IntSwissKnife` (result coerced to `i64`), `false` for `SwissKnife` (`f64`).
    pub is_integer: bool,
}

/// Shared shape of `Converter`/`IntConverter`: two formulas (`FormulaTo`/`FormulaFrom`) plus the
/// underlying register/feature node they wrap.
#[derive(Debug, Clone)]
pub struct ConverterNode {
    pub formula_to: Formula,
    pub formula_from: Formula,
    pub variables: Vec<(String, NodeId)>,
    pub value_link: NodeId,
    pub is_integer: bool,
}

#[derive(Debug, Clone)]
pub enum Node {
    Category(CategoryNode),
    Integer(NumericNode),
    Float(NumericNode),
    Boolean(BooleanNode),
    Enumeration(EnumerationNode),
    EnumEntry(EnumEntryNode),
    Command(CommandNode),
    /// A register holding a NUL-terminated string (`<StringReg>`), addressed/cached exactly
    /// like `IntReg` but decoded as text rather than an integer.
    StringReg(RegisterAccessSpec),
    /// A front-end string feature (`<String>`), confirmed necessary against the live
    /// C6-2040-GigE (`EventLogMessageText`, `ValueArrayCandidates`).
    String(StringSource),
    IntReg(RegisterAccessSpec),
    /// A register holding an IEEE-754 float bit pattern (`<FloatReg>`), 4 or 8 bytes.
    FloatReg(RegisterAccessSpec),
    Converter(ConverterNode),
    SwissKnife(SwissKnifeNode),
}

impl Node {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Node::Category(_) => "Category",
            Node::Integer(_) => "Integer",
            Node::Float(_) => "Float",
            Node::Boolean(_) => "Boolean",
            Node::Enumeration(_) => "Enumeration",
            Node::EnumEntry(_) => "EnumEntry",
            Node::Command(_) => "Command",
            Node::StringReg(_) => "StringReg",
            Node::String(_) => "String",
            Node::IntReg(_) => "IntReg",
            Node::FloatReg(_) => "FloatReg",
            Node::Converter(_) => "Converter",
            Node::SwissKnife(_) => "SwissKnife",
        }
    }
}

/// Encode/decode helpers for register byte <-> `i64` conversion, matching `ArvGcRegisterNode`'s
/// masked/signed/endianness algorithm. Bit numbering is taken directly from `LSB`/`MSB` without
/// the big-endian bit-reversal edge case the C reference applies for some vendor conventions —
/// a documented simplification, validated against the live camera's actual masked registers.
pub fn bytes_to_u64(bytes: &[u8], endianness: Endianness) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    match endianness {
        Endianness::Big => buf[8 - n..].copy_from_slice(&bytes[..n]),
        Endianness::Little => buf[..n].copy_from_slice(&bytes[..n]),
    }
    match endianness {
        Endianness::Big => u64::from_be_bytes(buf),
        Endianness::Little => u64::from_le_bytes(buf),
    }
}

pub fn u64_to_bytes(value: u64, len: usize, endianness: Endianness) -> Vec<u8> {
    let len = len.min(8);
    match endianness {
        Endianness::Big => value.to_be_bytes()[8 - len..].to_vec(),
        Endianness::Little => value.to_le_bytes()[..len].to_vec(),
    }
}

pub fn decode_int(bytes: &[u8], spec: &RegisterAccessSpec) -> i64 {
    let raw = bytes_to_u64(bytes, spec.endianness);
    match spec.bit_mask {
        Some((lsb, msb)) => {
            let width = (msb - lsb + 1) as u32;
            let mask: u64 = if width >= 64 { u64::MAX } else { ((1u64 << width) - 1) << lsb };
            let mut v = (raw & mask) >> lsb;
            if spec.sign == Sign::Signed && width < 64 && (v >> (width - 1)) & 1 == 1 {
                v |= u64::MAX << width;
            }
            v as i64
        }
        None => {
            let len_bits = (bytes.len().min(8) * 8) as u32;
            let mut v = raw;
            if spec.sign == Sign::Signed && len_bits > 0 && len_bits < 64 && (v >> (len_bits - 1)) & 1 == 1 {
                v |= u64::MAX << len_bits;
            }
            v as i64
        }
    }
}

/// Decode an IEEE-754 bit pattern (4 bytes -> `f32`, 8 bytes -> `f64`; anything else decodes as 0).
pub fn decode_float(bytes: &[u8], endianness: Endianness) -> f64 {
    match bytes.len() {
        4 => f32::from_bits(bytes_to_u64(bytes, endianness) as u32) as f64,
        8 => f64::from_bits(bytes_to_u64(bytes, endianness)),
        _ => 0.0,
    }
}

pub fn encode_float(value: f64, length: usize, endianness: Endianness) -> Vec<u8> {
    match length {
        4 => u64_to_bytes((value as f32).to_bits() as u64, 4, endianness),
        8 => u64_to_bytes(value.to_bits(), 8, endianness),
        _ => vec![0u8; length],
    }
}

pub fn encode_int(current_bytes: &[u8], value: i64, spec: &RegisterAccessSpec) -> Vec<u8> {
    let value_bits = value as u64;
    let new_raw = match spec.bit_mask {
        Some((lsb, msb)) => {
            let width = (msb - lsb + 1) as u32;
            let mask: u64 = if width >= 64 { u64::MAX } else { ((1u64 << width) - 1) << lsb };
            let current_raw = bytes_to_u64(current_bytes, spec.endianness);
            (current_raw & !mask) | ((value_bits << lsb) & mask)
        }
        None => value_bits,
    };
    u64_to_bytes(new_raw, spec.length as usize, spec.endianness)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(length: u32, endianness: Endianness, bit_mask: Option<(u8, u8)>, sign: Sign) -> RegisterAccessSpec {
        RegisterAccessSpec {
            length,
            endianness,
            bit_mask,
            sign,
            ..Default::default()
        }
    }

    #[test]
    fn plain_unsigned_big_endian_round_trip() {
        let spec = spec(4, Endianness::Big, None, Sign::Unsigned);
        let bytes = 0x1234_5678u32.to_be_bytes();
        assert_eq!(decode_int(&bytes, &spec), 0x1234_5678);
        assert_eq!(encode_int(&bytes, 0x1234_5678, &spec), bytes.to_vec());
    }

    #[test]
    fn plain_signed_value_sign_extends() {
        let spec = spec(4, Endianness::Big, None, Sign::Signed);
        let bytes = (-5i32).to_be_bytes();
        assert_eq!(decode_int(&bytes, &spec), -5);
    }

    #[test]
    fn masked_bitfield_unsigned() {
        // A 4-byte register, bits [7:4] hold a nibble value.
        let spec = spec(4, Endianness::Big, Some((4, 7)), Sign::Unsigned);
        let bytes = 0x0000_00A0u32.to_be_bytes(); // 0b1010_0000 -> nibble bits[7:4] = 0b1010 = 10
        assert_eq!(decode_int(&bytes, &spec), 10);
    }

    #[test]
    fn masked_bitfield_signed_sign_extends() {
        let spec = spec(4, Endianness::Big, Some((0, 3)), Sign::Signed);
        let bytes = 0x0000_000Fu32.to_be_bytes(); // low nibble = 0b1111 = -1 in 4-bit two's complement
        assert_eq!(decode_int(&bytes, &spec), -1);
    }

    #[test]
    fn masked_write_preserves_other_bits() {
        let spec = spec(4, Endianness::Big, Some((4, 7)), Sign::Unsigned);
        let current = 0xABu32.to_be_bytes(); // low nibble = B, high nibble = A
        let written = encode_int(&current, 0xC, &spec);
        assert_eq!(u32::from_be_bytes(written.try_into().unwrap()), 0xCB);
    }

    #[test]
    fn little_endian_round_trip() {
        let spec = spec(2, Endianness::Little, None, Sign::Unsigned);
        let bytes = 0x1234u16.to_le_bytes();
        assert_eq!(decode_int(&bytes, &spec), 0x1234);
    }

    #[test]
    fn float32_round_trip() {
        let bytes = encode_float(3.5, 4, Endianness::Big);
        assert_eq!(decode_float(&bytes, Endianness::Big), 3.5);
    }

    #[test]
    fn float64_round_trip() {
        let original = 12_345.678_9;
        let bytes = encode_float(original, 8, Endianness::Little);
        assert!((decode_float(&bytes, Endianness::Little) - original).abs() < 1e-12);
    }
}
