use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::access::RegisterAccess;
use crate::dom::{self, XmlDom};
use crate::error::{GenIcamError, Result};
use crate::formula::{Formula, Value};
use crate::node::{
    AddressTerm, BooleanNode, Cachable, IndexOffset, CategoryNode, CommandNode, ConverterNode, Endianness,
    EnumEntryNode, EnumerationNode, Node, NodeId, NumericNode, RegisterAccessSpec, Sign,
    StringSource, SwissKnifeNode, ValueSource,
};

struct CachedValue {
    bytes: Vec<u8>,
    filled_at: u64,
    /// The address the bytes were read from or written to. A register whose address follows
    /// a selector (`pIndex`/`pAddress`) moves when the selector changes, and documents rarely
    /// list the selector as a `pInvalidator`, so a cached value is only valid at this address.
    address: u64,
}

/// `(pVariable bindings, Constant name->text, Expression name->text)` collected from a
/// `SwissKnife`/`Converter`-shaped element.
type VariablesConstantsSubExprs = (Vec<(String, NodeId)>, Vec<(String, String)>, Vec<(String, String)>);

/// Identification attributes of the document's `<RegisterDescription>` root element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterDescription {
    pub vendor_name: String,
    pub model_name: String,
    /// `(SchemaMajorVersion, SchemaMinorVersion, SchemaSubMinorVersion)`. Missing or unparsable
    /// components are 0, as in Aravis, so a document without them counts as pre-1.1 schema.
    pub schema_version: (u32, u32, u32),
}

impl RegisterDescription {
    fn from_root(dom: &XmlDom) -> Self {
        let root = dom.get(dom.root);
        let attr = |name: &str| root.attrs.get(name).map(|v| v.trim().to_string()).unwrap_or_default();
        let version = |name: &str| attr(name).parse::<u32>().unwrap_or(0);
        Self {
            vendor_name: attr("VendorName"),
            model_name: attr("ModelName"),
            schema_version: (
                version("SchemaMajorVersion"),
                version("SchemaMinorVersion"),
                version("SchemaSubMinorVersion"),
            ),
        }
    }
}

/// One entry of an `Enumeration`, as reported by [`GenApiTree::feature_info`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumEntryState {
    pub name: String,
    pub value: i64,
    /// The entry's own `pIsImplemented`/`pIsAvailable` predicates hold. A device lists entries it
    /// doesn't support this way (e.g. gain steps a sensor variant lacks).
    pub available: bool,
}

/// What a GenICam browser shows next to a feature besides its value: whether it can be used
/// right now, and the range and unit to offer. Evaluated in one go by
/// [`GenApiTree::feature_info`].
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureInfo {
    /// The node kind, as [`GenApiTree::node_kind`] names it.
    pub kind: &'static str,
    /// All `pIsImplemented`/`pIsAvailable` predicates hold.
    pub available: bool,
    /// Some `pIsLocked` predicate holds: the feature is readable but must not be written now.
    pub locked: bool,
    /// Evaluated `Min`/`pMin`, `Max`/`pMax` and `Inc`/`pInc`, in the feature's own type (`Int`
    /// for an `Integer`, `Float` for a `Float`). `None` when the document gives none, the feature
    /// is unavailable, or the bound can't be evaluated or isn't finite.
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub inc: Option<Value>,
    pub unit: Option<String>,
    pub representation: Option<String>,
    /// The entries of an `Enumeration`, in document order; `None` for any other kind.
    pub entries: Option<Vec<EnumEntryState>>,
}

/// The parsed GenICam node tree plus register cache/invalidation state. Transport-agnostic:
/// every method that touches the wire takes a `&mut impl RegisterAccess`.
pub struct GenApiTree {
    register_description: RegisterDescription,
    nodes: Vec<Node>,
    /// `pIsImplemented`/`pIsAvailable` predicates per node, for [`GenApiTree::is_available`].
    availability: HashMap<NodeId, Vec<NodeId>>,
    /// `pIsLocked` predicates per node, for [`GenApiTree::is_locked`].
    locked: HashMap<NodeId, Vec<NodeId>>,
    names: HashMap<String, NodeId>,
    cache: Vec<RefCell<Option<CachedValue>>>,
    epoch: Vec<Cell<u64>>,
    global_tick: Cell<u64>,
    caching_enabled: Cell<bool>,
}

const CONSTRUCTIBLE_TAGS: &[&str] = &[
    "Category",
    "Integer",
    "Float",
    "Boolean",
    "Enumeration",
    "EnumEntry",
    "Command",
    "StringReg",
    "String",
    "IntReg",
    "MaskedIntReg",
    "FloatReg",
    "Converter",
    "IntConverter",
    "SwissKnife",
    "IntSwissKnife",
    // Each `<StructEntry>` is a named bit field of its enclosing `<StructReg>`, i.e. a
    // `MaskedIntReg` that inherits the register elements (Address, Length, Endianess, ...) of its
    // parent; confirmed necessary against the live C6-2040-GigE's `Scan3dCapabilities*Reg`
    // entries. The `StructReg` itself is only a container and has no node of its own.
    "StructEntry",
    // Out-of-scope node kinds (file access, GenICam events) that still need to *resolve* as
    // named references — confirmed necessary against the live camera's `FileAccessBuffer`
    // (a plain `<Register>`) and per-event `<Port>` nodes — even though we don't implement their
    // full semantics.
    "Register",
    "Port",
];

fn parse_int_literal(text: &str) -> Result<i64> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("true") {
        return Ok(1);
    }
    if t.eq_ignore_ascii_case("false") {
        return Ok(0);
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| GenIcamError::Xml(format!("invalid hex literal '{text}'")))
    } else {
        t.parse::<i64>().map_err(|_| GenIcamError::Xml(format!("invalid int literal '{text}'")))
    }
}

fn parse_float_literal(text: &str) -> Result<f64> {
    text.trim()
        .parse::<f64>()
        .map_err(|_| GenIcamError::Xml(format!("invalid float literal '{text}'")))
}

fn parse_u64_literal(text: &str) -> Result<u64> {
    Ok(parse_int_literal(text)? as u64)
}

struct Builder<'a> {
    dom: &'a XmlDom,
    names: HashMap<String, NodeId>,
    dom_to_node: HashMap<usize, NodeId>,
    /// `StructEntry` DOM index -> DOM index of its enclosing `StructReg`.
    struct_parent: HashMap<usize, usize>,
    /// `<Port>` name -> its `<ChunkID>`, for the ports that address chunk data.
    chunk_ports: HashMap<String, u32>,
}

impl<'a> Builder<'a> {
    fn resolve(&self, name: &str) -> Result<NodeId> {
        self.names
            .get(name.trim())
            .copied()
            .ok_or_else(|| GenIcamError::DanglingReference(name.trim().to_string()))
    }

    fn resolve_child_ref(&self, idx: usize, tag: &str) -> Result<Option<NodeId>> {
        match self.dom.child(idx, tag) {
            Some(c) => Ok(Some(self.resolve(&self.dom.get(c).text)?)),
            None => Ok(None),
        }
    }

    fn value_source_int(&self, idx: usize) -> Result<ValueSource> {
        if let Some(target) = self.resolve_child_ref(idx, "pValue")? {
            return Ok(ValueSource::PValue(target));
        }
        if let Some(indexed) = self.indexed_source(idx, true)? {
            return Ok(indexed);
        }
        if let Some(v) = self.dom.child(idx, "Value") {
            return Ok(ValueSource::LiteralInt(parse_int_literal(&self.dom.get(v).text)?));
        }
        Ok(ValueSource::LiteralInt(0))
    }

    fn value_source_float(&self, idx: usize) -> Result<ValueSource> {
        if let Some(target) = self.resolve_child_ref(idx, "pValue")? {
            return Ok(ValueSource::PValue(target));
        }
        if let Some(indexed) = self.indexed_source(idx, false)? {
            return Ok(indexed);
        }
        if let Some(v) = self.dom.child(idx, "Value") {
            return Ok(ValueSource::LiteralFloat(parse_float_literal(&self.dom.get(v).text)?));
        }
        Ok(ValueSource::LiteralFloat(0.0))
    }

    fn literal(text: &str, is_int: bool) -> Result<ValueSource> {
        Ok(if is_int {
            ValueSource::LiteralInt(parse_int_literal(text)?)
        } else {
            ValueSource::LiteralFloat(parse_float_literal(text)?)
        })
    }

    /// A `<pIndex>` value table (see [`ValueSource::Indexed`]), or `None` without a `pIndex`.
    fn indexed_source(&self, idx: usize, is_int: bool) -> Result<Option<ValueSource>> {
        let Some(index) = self.resolve_child_ref(idx, "pIndex")? else {
            return Ok(None);
        };
        let key = |e: usize| -> Result<i64> {
            let el = self.dom.get(e);
            let text = el
                .attrs
                .get("Index")
                .ok_or_else(|| GenIcamError::Xml(format!("{} missing Index attribute", el.tag)))?;
            parse_int_literal(text)
        };
        let mut entries = Vec::new();
        for e in self.dom.children_with_tag(idx, "ValueIndexed") {
            entries.push((key(e)?, Self::literal(&self.dom.get(e).text, is_int)?));
        }
        for e in self.dom.children_with_tag(idx, "pValueIndexed") {
            entries.push((key(e)?, ValueSource::PValue(self.resolve(&self.dom.get(e).text)?)));
        }
        let default = match self.resolve_child_ref(idx, "pValueDefault")? {
            Some(target) => ValueSource::PValue(target),
            None => match self.dom.child(idx, "ValueDefault") {
                Some(v) => Self::literal(&self.dom.get(v).text, is_int)?,
                None => Self::literal("0", is_int)?,
            },
        };
        Ok(Some(ValueSource::Indexed {
            index,
            entries,
            default: Box::new(default),
        }))
    }

    /// `<{tag}>` or `<p{tag}>` (`Min`, `Max`, `Inc`); the pointer wins. A pointer to a node this
    /// crate can't build is ignored rather than failing the whole document over a bound, which
    /// only ever served as a UI hint before.
    fn numeric_bound(&self, idx: usize, tag: &str, is_int: bool) -> Result<Option<ValueSource>> {
        if let Some(p) = self.dom.child(idx, &format!("p{tag}")) {
            return Ok(self.names.get(self.dom.get(p).text.trim()).map(|&id| ValueSource::PValue(id)));
        }
        match self.dom.child(idx, tag) {
            Some(c) => Ok(Some(Self::literal(&self.dom.get(c).text, is_int)?)),
            None => Ok(None),
        }
    }

    fn child_text(&self, idx: usize, tag: &str) -> Option<String> {
        self.dom
            .child(idx, tag)
            .map(|c| self.dom.get(c).text.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    fn numeric_node(&self, idx: usize, is_int: bool) -> Result<NumericNode> {
        Ok(NumericNode {
            value: if is_int { self.value_source_int(idx)? } else { self.value_source_float(idx)? },
            min: self.numeric_bound(idx, "Min", is_int)?,
            max: self.numeric_bound(idx, "Max", is_int)?,
            inc: self.numeric_bound(idx, "Inc", is_int)?,
            unit: self.child_text(idx, "Unit"),
            representation: self.child_text(idx, "Representation"),
        })
    }

    fn register_spec(&self, idx: usize) -> Result<RegisterAccessSpec> {
        // A `StructEntry` takes every register element it doesn't set itself from its
        // `StructReg`; for any other register node there is no parent to fall back to.
        let parent = self.struct_parent.get(&idx).copied();
        let child = |tag: &str| self.dom.child(idx, tag).or_else(|| parent.and_then(|p| self.dom.child(p, tag)));
        let children = |tag: &str| {
            let mut all = parent.map(|p| self.dom.children_with_tag(p, tag)).unwrap_or_default();
            all.extend(self.dom.children_with_tag(idx, tag));
            all
        };

        let mut address_terms = Vec::new();
        for a in children("Address") {
            address_terms.push(AddressTerm::Literal(parse_u64_literal(&self.dom.get(a).text)?));
        }
        for a in children("pAddress") {
            address_terms.push(AddressTerm::PAddress(self.resolve(&self.dom.get(a).text)?));
        }
        for i in children("pIndex") {
            let el = self.dom.get(i);
            let offset = match (el.attrs.get("Offset"), el.attrs.get("pOffset")) {
                (Some(offset), _) => IndexOffset::Literal(parse_u64_literal(offset)?),
                (None, Some(node)) => IndexOffset::PNode(self.resolve(node)?),
                (None, None) => IndexOffset::RegisterLength,
            };
            address_terms.push(AddressTerm::Index {
                index: self.resolve(&el.text)?,
                offset,
            });
        }
        let chunk_id = child("pPort").and_then(|p| self.chunk_ports.get(self.dom.get(p).text.trim()).copied());

        let length = match child("Length") {
            Some(c) => parse_u64_literal(&self.dom.get(c).text)? as u32,
            None => 4,
        };

        let endianness = match child("Endianess").map(|c| self.dom.get(c).text.trim().to_string()) {
            Some(s) if s.eq_ignore_ascii_case("LittleEndian") => Endianness::Little,
            _ => Endianness::Big,
        };

        let sign = match child("Sign").map(|c| self.dom.get(c).text.trim().to_string()) {
            Some(s) if s.eq_ignore_ascii_case("Signed") => Sign::Signed,
            _ => Sign::Unsigned,
        };

        // GenICam XML bit numbering for a `BigEndian` masked register is given in wire/network
        // bit order, which is the *reverse* of the register's numeric bit position — confirmed
        // against every multi- and single-bit `MaskedIntReg` in the live C5-2040-GigE's XML
        // (e.g. `<LSB>31</LSB><MSB>16</MSB>` on a 4-byte register means bits [15:0], and
        // `<LSB>31</LSB><MSB>31</MSB>` — the standard "SupportedPersistentIP" flag — means bit
        // 0). `LittleEndian` registers (also present in this XML, for chunk data) use the XML
        // values directly, unreversed. Without this transform, `xml_lsb > xml_msb` (the common
        // case for BigEndian fields) underflows the `msb - lsb` width computation in
        // `node::decode_int`/`encode_int`.
        let reverse_bit = |xml_bit: u8| -> u8 { (8 * length - 1).saturating_sub(xml_bit as u32) as u8 };
        let bit_mask = if let Some(bit) = child("Bit") {
            let n = parse_u64_literal(&self.dom.get(bit).text)? as u8;
            let n = if endianness == Endianness::Big { reverse_bit(n) } else { n };
            Some((n, n))
        } else {
            let lsb = child("LSB").map(|c| parse_u64_literal(&self.dom.get(c).text)).transpose()?;
            let msb = child("MSB").map(|c| parse_u64_literal(&self.dom.get(c).text)).transpose()?;
            match (lsb, msb) {
                (Some(l), Some(m)) => {
                    let (l, m) = (l as u8, m as u8);
                    if endianness == Endianness::Big {
                        Some((reverse_bit(l), reverse_bit(m)))
                    } else {
                        Some((l, m))
                    }
                }
                _ => None,
            }
        };

        let cachable = match child("Cachable").map(|c| self.dom.get(c).text.trim().to_string()) {
            Some(s) if s.eq_ignore_ascii_case("WriteThrough") => Cachable::WriteThrough,
            Some(s) if s.eq_ignore_ascii_case("WriteAround") => Cachable::WriteAround,
            _ => Cachable::NoCache,
        };

        let writable = !matches!(
            child("AccessMode").map(|c| self.dom.get(c).text.trim().to_string()),
            Some(ref s) if s.eq_ignore_ascii_case("RO")
        );

        let mut invalidators = Vec::new();
        for inv in children("pInvalidator") {
            invalidators.push(self.resolve(&self.dom.get(inv).text)?);
        }
        // Sibling entries share one register, so a write through any of them must invalidate
        // the cached bytes of all the others.
        if let Some(p) = parent {
            for sibling in self.dom.children_with_tag(p, "StructEntry") {
                if sibling != idx {
                    if let Some(&id) = self.dom_to_node.get(&sibling) {
                        invalidators.push(id);
                    }
                }
            }
        }

        Ok(RegisterAccessSpec {
            address_terms,
            length,
            endianness,
            bit_mask,
            sign,
            cachable,
            invalidators,
            writable,
            chunk_id,
        })
    }

    /// The `pIsImplemented`/`pIsAvailable` predicates of the element at `idx`.
    fn availability(&self, idx: usize) -> Result<Vec<NodeId>> {
        let mut predicates = Vec::new();
        for tag in ["pIsImplemented", "pIsAvailable"] {
            for p in self.dom.children_with_tag(idx, tag) {
                predicates.push(self.resolve(&self.dom.get(p).text)?);
            }
        }
        Ok(predicates)
    }

    /// The `pIsLocked` predicates of the element at `idx`. Lenient like
    /// [`Builder::numeric_bound`]: a dangling predicate is skipped, not an error.
    fn lock_predicates(&self, idx: usize) -> Vec<NodeId> {
        self.dom
            .children_with_tag(idx, "pIsLocked")
            .into_iter()
            .filter_map(|p| self.names.get(self.dom.get(p).text.trim()).copied())
            .collect()
    }

    fn variables_constants_subexprs(&self, idx: usize) -> Result<VariablesConstantsSubExprs> {
        let mut variables = Vec::new();
        for v in self.dom.children_with_tag(idx, "pVariable") {
            let el = self.dom.get(v);
            let var_name = el
                .attrs
                .get("Name")
                .cloned()
                .ok_or_else(|| GenIcamError::Xml("pVariable missing Name attribute".to_string()))?;
            variables.push((var_name, self.resolve(&el.text)?));
        }
        let mut constants = Vec::new();
        for c in self.dom.children_with_tag(idx, "Constant") {
            let el = self.dom.get(c);
            let name = el
                .attrs
                .get("Name")
                .cloned()
                .ok_or_else(|| GenIcamError::Xml("Constant missing Name attribute".to_string()))?;
            constants.push((name, el.text.trim().to_string()));
        }
        let mut sub_expressions = Vec::new();
        for e in self.dom.children_with_tag(idx, "Expression") {
            let el = self.dom.get(e);
            let name = el
                .attrs
                .get("Name")
                .cloned()
                .ok_or_else(|| GenIcamError::Xml("Expression missing Name attribute".to_string()))?;
            sub_expressions.push((name, el.text.trim().to_string()));
        }
        Ok((variables, constants, sub_expressions))
    }

    fn build_category(&self, idx: usize) -> Result<CategoryNode> {
        let mut children = Vec::new();
        for p in self.dom.children_with_tag(idx, "pFeature") {
            children.push(self.resolve(&self.dom.get(p).text)?);
        }
        Ok(CategoryNode { children })
    }

    fn build_enumeration(&self, idx: usize) -> Result<EnumerationNode> {
        let mut entries = Vec::new();
        for e in self.dom.children_with_tag(idx, "EnumEntry") {
            let name = self.dom.get(e).attrs.get("Name").cloned().ok_or_else(|| {
                GenIcamError::Xml("EnumEntry missing Name attribute".to_string())
            })?;
            let node_id = *self.dom_to_node.get(&e).expect("EnumEntry reserved in pass 1");
            entries.push((name, node_id));
        }
        Ok(EnumerationNode {
            entries,
            value: self.value_source_int(idx)?,
        })
    }

    fn build_enum_entry(&self, idx: usize) -> Result<EnumEntryNode> {
        let v = self
            .dom
            .child(idx, "Value")
            .ok_or_else(|| GenIcamError::Xml("EnumEntry missing Value".to_string()))?;
        Ok(EnumEntryNode {
            value: parse_int_literal(&self.dom.get(v).text)?,
        })
    }

    fn build_command(&self, idx: usize) -> Result<CommandNode> {
        let command_value = match self.dom.child(idx, "CommandValue") {
            Some(c) => parse_int_literal(&self.dom.get(c).text)?,
            None => 1,
        };
        Ok(CommandNode {
            value: self.value_source_int(idx)?,
            command_value,
        })
    }

    fn build_swiss_knife(&self, idx: usize, is_integer: bool) -> Result<SwissKnifeNode> {
        let (variables, constants, sub_expressions) = self.variables_constants_subexprs(idx)?;
        let formula_idx = self
            .dom
            .child(idx, "Formula")
            .ok_or_else(|| GenIcamError::Xml("SwissKnife missing Formula".to_string()))?;
        let formula = Formula::parse(&self.dom.get(formula_idx).text, &constants, &sub_expressions)?;
        Ok(SwissKnifeNode {
            formula,
            variables,
            is_integer,
        })
    }

    fn build_converter(&self, idx: usize, is_integer: bool) -> Result<ConverterNode> {
        let (variables, constants, sub_expressions) = self.variables_constants_subexprs(idx)?;
        let value_link = self
            .resolve_child_ref(idx, "pValue")?
            .ok_or_else(|| GenIcamError::Xml("Converter missing pValue".to_string()))?;
        let to_idx = self
            .dom
            .child(idx, "FormulaTo")
            .ok_or_else(|| GenIcamError::Xml("Converter missing FormulaTo".to_string()))?;
        let from_idx = self
            .dom
            .child(idx, "FormulaFrom")
            .ok_or_else(|| GenIcamError::Xml("Converter missing FormulaFrom".to_string()))?;
        Ok(ConverterNode {
            formula_to: Formula::parse(&self.dom.get(to_idx).text, &constants, &sub_expressions)?,
            formula_from: Formula::parse(&self.dom.get(from_idx).text, &constants, &sub_expressions)?,
            variables,
            value_link,
            is_integer,
        })
    }

    fn build_node(&self, idx: usize) -> Result<Node> {
        Ok(match self.dom.get(idx).tag.as_str() {
            "Category" => Node::Category(self.build_category(idx)?),
            "Integer" => Node::Integer(self.numeric_node(idx, true)?),
            "Float" => Node::Float(self.numeric_node(idx, false)?),
            "Boolean" => Node::Boolean(BooleanNode {
                value: self.value_source_int(idx)?,
            }),
            "Enumeration" => Node::Enumeration(self.build_enumeration(idx)?),
            "EnumEntry" => Node::EnumEntry(self.build_enum_entry(idx)?),
            "Command" => Node::Command(self.build_command(idx)?),
            "StringReg" => Node::StringReg(self.register_spec(idx)?),
            "String" => Node::String(match self.resolve_child_ref(idx, "pValue")? {
                Some(target) => StringSource::PValue(target),
                None => StringSource::Literal(
                    self.dom.child(idx, "Value").map(|v| self.dom.get(v).text.clone()).unwrap_or_default(),
                ),
            }),
            "IntReg" | "MaskedIntReg" | "StructEntry" => Node::IntReg(self.register_spec(idx)?),
            "FloatReg" => Node::FloatReg(self.register_spec(idx)?),
            "Converter" => Node::Converter(self.build_converter(idx, false)?),
            "IntConverter" => Node::Converter(self.build_converter(idx, true)?),
            "SwissKnife" => Node::SwissKnife(self.build_swiss_knife(idx, false)?),
            "IntSwissKnife" => Node::SwissKnife(self.build_swiss_knife(idx, true)?),
            // Plain <Register> (e.g. file-access buffers): same address/length/cache mechanics
            // as IntReg, just addressed generically rather than through a typed feature wrapper.
            "Register" => Node::IntReg(self.register_spec(idx)?),
            // <Port> (event/file-access stream endpoints): out of scope, kept as an inert,
            // resolvable placeholder so nothing that references one by name fails to parse.
            "Port" => Node::Category(CategoryNode::default()),
            other => unreachable!("unrecognized constructible tag {other}"),
        })
    }
}

impl GenApiTree {
    pub fn parse(xml: &str) -> Result<Self> {
        let dom = dom::parse(xml)?;

        // Pass 1: reserve a NodeId (with a placeholder value) for every constructible element,
        // so forward references (pFeature/pValue/pAddress/pVariable) resolve regardless of
        // document order.
        let mut nodes = Vec::new();
        let mut names = HashMap::new();
        let mut dom_to_node = HashMap::new();
        let mut struct_parent = HashMap::new();
        let mut chunk_ports = HashMap::new();
        for (idx, el) in dom.elements.iter().enumerate() {
            if el.tag == "Port" {
                if let (Some(name), Some(c)) = (el.attrs.get("Name"), dom.child(idx, "ChunkID")) {
                    // ChunkID is hexadecimal, with or without a 0x prefix.
                    let text = dom.get(c).text.trim();
                    let hex = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
                    let id = u32::from_str_radix(hex, 16)
                        .map_err(|_| GenIcamError::Xml(format!("invalid ChunkID '{text}' on port '{name}'")))?;
                    chunk_ports.insert(name.clone(), id);
                }
            }
            // A `StructReg` always precedes its entries in document order, so every entry's
            // parent is known by the time the loop reaches it.
            if el.tag == "StructReg" {
                for entry in dom.children_with_tag(idx, "StructEntry") {
                    struct_parent.insert(entry, idx);
                }
            }
            if !CONSTRUCTIBLE_TAGS.contains(&el.tag.as_str()) {
                continue;
            }
            // A `StructEntry` outside a `StructReg` has no register to address.
            if el.tag == "StructEntry" && !struct_parent.contains_key(&idx) {
                continue;
            }
            let id = NodeId(nodes.len() as u32);
            nodes.push(Node::Category(CategoryNode::default()));
            dom_to_node.insert(idx, id);
            if el.tag != "EnumEntry" {
                if let Some(name) = el.attrs.get("Name") {
                    names.insert(name.clone(), id);
                }
            }
        }

        let n = nodes.len();
        let builder = Builder {
            dom: &dom,
            names,
            dom_to_node,
            struct_parent,
            chunk_ports,
        };

        // Pass 2: construct the real node value for every reserved slot, now that all names are
        // known. `Builder::build_node` only reads `dom`/`names`/`dom_to_node`, never
        // `builder.nodes` — that field exists solely to size `built_nodes` below.
        let mut built_nodes: Vec<Node> = (0..n).map(|_| Node::Category(CategoryNode::default())).collect();
        let mut availability = HashMap::new();
        let mut locked = HashMap::new();
        for (&idx, &id) in &builder.dom_to_node {
            built_nodes[id.index()] = builder.build_node(idx)?;
            let predicates = builder.availability(idx)?;
            if !predicates.is_empty() {
                availability.insert(id, predicates);
            }
            let lock = builder.lock_predicates(idx);
            if !lock.is_empty() {
                locked.insert(id, lock);
            }
        }

        let n = built_nodes.len();
        Ok(Self {
            register_description: RegisterDescription::from_root(&dom),
            nodes: built_nodes,
            availability,
            locked,
            names: builder.names,
            cache: (0..n).map(|_| RefCell::new(None)).collect(),
            epoch: (0..n).map(|_| Cell::new(0)).collect(),
            global_tick: Cell::new(0),
            caching_enabled: Cell::new(true),
        })
    }

    /// The document's `<RegisterDescription>` attributes (vendor, model, schema version).
    pub fn register_description(&self) -> &RegisterDescription {
        &self.register_description
    }

    /// Whether `name` is currently implemented and available, i.e. all of its
    /// `pIsImplemented`/`pIsAvailable` predicates evaluate non-zero (a feature without any is
    /// always available). Tools such as GenICam browsers show unavailable features as "-".
    /// Evaluating a predicate may read device registers.
    pub fn is_available(&self, io: &mut impl RegisterAccess, name: &str) -> Result<bool> {
        let id = self.node_id(name)?;
        self.available_by_id(io, id, name)
    }

    fn available_by_id(&self, io: &mut impl RegisterAccess, id: NodeId, name: &str) -> Result<bool> {
        for &predicate in self.availability.get(&id).into_iter().flatten() {
            if self.eval_node(io, predicate, name)?.as_i64() == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether `name` is currently locked, i.e. any of its `pIsLocked` predicates evaluates
    /// non-zero (a feature without any is never locked). A locked feature can be read but not
    /// written. Predicates on the GenTL-side `TLParamsLocked` evaluate to that node's literal,
    /// as this crate never sets it. Evaluating a predicate may read device registers.
    pub fn is_locked(&self, io: &mut impl RegisterAccess, name: &str) -> Result<bool> {
        let id = self.node_id(name)?;
        self.locked_by_id(io, id, name)
    }

    fn locked_by_id(&self, io: &mut impl RegisterAccess, id: NodeId, name: &str) -> Result<bool> {
        for &predicate in self.locked.get(&id).into_iter().flatten() {
            if self.eval_node(io, predicate, name)?.as_i64() != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether any `pIsLocked` predicate of `name` depends on node `target`, directly or through
    /// the formulas, values and addresses it is computed from. Structural: no device I/O.
    ///
    /// Asked with `"TLParamsLocked"` it tells which features the device locks while acquiring
    /// (GenTL sets that node during acquisition). This crate never sets it, so such features
    /// report unlocked and accept writes, but a device may only apply them at the next
    /// acquisition start: the live C6-2040-GigE stores a new `ExposureTime` mid-acquisition and
    /// keeps exposing with the old one.
    pub fn lock_depends_on(&self, name: &str, target: &str) -> Result<bool> {
        let id = self.node_id(name)?;
        let Ok(target) = self.node_id(target) else {
            return Ok(false);
        };
        let mut seen = std::collections::HashSet::new();
        let mut stack: Vec<NodeId> = self.locked.get(&id).cloned().unwrap_or_default();
        while let Some(next) = stack.pop() {
            if next == target {
                return Ok(true);
            }
            if seen.insert(next) {
                self.node_references(next, &mut stack);
            }
        }
        Ok(false)
    }

    /// The nodes `id`'s value is computed from (not its invalidators or bounds).
    fn node_references(&self, id: NodeId, out: &mut Vec<NodeId>) {
        fn value_refs(vs: &ValueSource, out: &mut Vec<NodeId>) {
            match vs {
                ValueSource::LiteralInt(_) | ValueSource::LiteralFloat(_) => {}
                ValueSource::PValue(target) => out.push(*target),
                ValueSource::Indexed { index, entries, default } => {
                    out.push(*index);
                    for (_, v) in entries {
                        value_refs(v, out);
                    }
                    value_refs(default, out);
                }
            }
        }
        fn address_refs(spec: &RegisterAccessSpec, out: &mut Vec<NodeId>) {
            for term in &spec.address_terms {
                match term {
                    AddressTerm::Literal(_) => {}
                    AddressTerm::PAddress(target) => out.push(*target),
                    AddressTerm::Index { index, offset } => {
                        out.push(*index);
                        if let IndexOffset::PNode(node) = offset {
                            out.push(*node);
                        }
                    }
                }
            }
        }
        match &self.nodes[id.index()] {
            Node::Integer(n) | Node::Float(n) => value_refs(&n.value, out),
            Node::Boolean(n) => value_refs(&n.value, out),
            Node::Enumeration(e) => value_refs(&e.value, out),
            Node::Command(c) => value_refs(&c.value, out),
            Node::SwissKnife(sk) => out.extend(sk.variables.iter().map(|(_, v)| *v)),
            Node::Converter(c) => {
                out.extend(c.variables.iter().map(|(_, v)| *v));
                out.push(c.value_link);
            }
            Node::IntReg(spec) | Node::FloatReg(spec) | Node::StringReg(spec) => address_refs(spec, out),
            Node::String(StringSource::PValue(target)) => out.push(*target),
            Node::String(StringSource::Literal(_)) | Node::Category(_) | Node::EnumEntry(_) => {}
        }
    }

    /// Availability, lock state, range, unit and (for an `Enumeration`) per-entry availability
    /// of `name`, for presenting it in a UI. Only an unknown `name` is an error: a predicate that
    /// can't be evaluated counts as available/unlocked, and a bound that can't be evaluated is
    /// `None`, so one odd node never hides the rest of what is known.
    pub fn feature_info(&self, io: &mut impl RegisterAccess, name: &str) -> Result<FeatureInfo> {
        let id = self.node_id(name)?;
        let node = &self.nodes[id.index()];
        let available = self.available_by_id(io, id, name).unwrap_or(true);
        let locked = self.locked_by_id(io, id, name).unwrap_or(false);

        let mut info = FeatureInfo {
            kind: node.kind_name(),
            available,
            locked,
            min: None,
            max: None,
            inc: None,
            unit: None,
            representation: None,
            entries: None,
        };
        match node {
            Node::Integer(n) | Node::Float(n) => {
                let is_int = matches!(node, Node::Integer(_));
                if available {
                    let mut bound = |vs: &Option<ValueSource>| -> Option<Value> {
                        let v = self.eval_value_source(io, vs.as_ref()?, name).ok()?;
                        if is_int {
                            Some(Value::Int(v.as_i64()))
                        } else {
                            Some(Value::Float(v.as_f64())).filter(|v| v.as_f64().is_finite())
                        }
                    };
                    info.min = bound(&n.min);
                    info.max = bound(&n.max);
                    info.inc = bound(&n.inc);
                }
                info.unit = n.unit.clone();
                info.representation = n.representation.clone();
            }
            Node::Enumeration(e) => {
                let mut entries = Vec::with_capacity(e.entries.len());
                for (entry_name, entry_id) in &e.entries {
                    let Node::EnumEntry(entry) = &self.nodes[entry_id.index()] else {
                        continue;
                    };
                    entries.push(EnumEntryState {
                        name: entry_name.clone(),
                        value: entry.value,
                        available: self.available_by_id(io, *entry_id, name).unwrap_or(true),
                    });
                }
                info.entries = Some(entries);
            }
            _ => {}
        }
        Ok(info)
    }

    pub fn node_id(&self, name: &str) -> Result<NodeId> {
        self.names.get(name).copied().ok_or_else(|| GenIcamError::NotFound(name.to_string()))
    }

    pub fn node_kind(&self, id: NodeId) -> &'static str {
        self.nodes[id.index()].kind_name()
    }

    pub fn feature_names(&self) -> impl Iterator<Item = &str> {
        self.names.keys().map(|s| s.as_str())
    }

    pub fn category_children(&self, name: &str) -> Result<Vec<String>> {
        let id = self.node_id(name)?;
        match &self.nodes[id.index()] {
            Node::Category(c) => Ok(c
                .children
                .iter()
                .filter_map(|child_id| self.names.iter().find(|(_, v)| **v == *child_id).map(|(k, _)| k.clone()))
                .collect()),
            other => Err(GenIcamError::TypeMismatch {
                name: name.to_string(),
                expected: "Category",
                found: other.kind_name(),
            }),
        }
    }

    /// Drop every cached register value. For callers that change device state behind the
    /// tree's back (raw register writes), which its invalidator tracking cannot see.
    pub fn invalidate_cache(&self) {
        for entry in &self.cache {
            *entry.borrow_mut() = None;
        }
    }

    pub fn set_caching_enabled(&self, enabled: bool) {
        self.caching_enabled.set(enabled);
    }

    fn bump_epoch(&self, id: NodeId) {
        let t = self.global_tick.get() + 1;
        self.global_tick.set(t);
        self.epoch[id.index()].set(t);
    }

    fn read_register_bytes(
        &self,
        io: &mut impl RegisterAccess,
        id: NodeId,
        spec: &RegisterAccessSpec,
        context_name: &str,
    ) -> Result<Vec<u8>> {
        // Chunk registers describe the buffer being inspected, never the device, so they are
        // neither cached nor read from device memory.
        if let Some(chunk_id) = spec.chunk_id {
            let address = self.resolve_address(io, spec, context_name)?;
            return io
                .read_chunk(chunk_id, address, spec.length as usize)
                .map_err(|e| GenIcamError::Io(e.to_string()));
        }
        let address = self.resolve_address(io, spec, context_name)?;
        if spec.cachable != Cachable::NoCache && self.caching_enabled.get() {
            let cached = self.cache[id.index()].borrow();
            if let Some(c) = cached.as_ref() {
                let valid = c.address == address
                    && spec
                        .invalidators
                        .iter()
                        .all(|inv| self.epoch[inv.index()].get() <= c.filled_at);
                if valid {
                    return Ok(c.bytes.clone());
                }
            }
        }
        let bytes = io
            .read_memory(address, spec.length as usize)
            .map_err(|e| GenIcamError::Io(e.to_string()))?;
        if spec.cachable != Cachable::NoCache {
            *self.cache[id.index()].borrow_mut() = Some(CachedValue {
                bytes: bytes.clone(),
                filled_at: self.global_tick.get(),
                address,
            });
        }
        Ok(bytes)
    }

    fn write_register_bytes(
        &self,
        io: &mut impl RegisterAccess,
        id: NodeId,
        spec: &RegisterAccessSpec,
        new_bytes: Vec<u8>,
        context_name: &str,
    ) -> Result<()> {
        if !spec.writable || spec.chunk_id.is_some() {
            return Err(GenIcamError::NotWritable(context_name.to_string()));
        }
        let address = self.resolve_address(io, spec, context_name)?;
        io.write_memory(address, &new_bytes).map_err(|e| GenIcamError::Io(e.to_string()))?;
        self.bump_epoch(id);
        *self.cache[id.index()].borrow_mut() = if spec.cachable == Cachable::WriteThrough {
            Some(CachedValue {
                bytes: new_bytes,
                filled_at: self.global_tick.get(),
                address,
            })
        } else {
            None
        };
        Ok(())
    }

    fn resolve_address(&self, io: &mut impl RegisterAccess, spec: &RegisterAccessSpec, context_name: &str) -> Result<u64> {
        let mut addr = 0u64;
        for term in &spec.address_terms {
            // Wrapping: terms come from device values and formulas, and a bogus one must yield a
            // bad address (which the device rejects), not an overflow panic.
            addr = addr.wrapping_add(match term {
                AddressTerm::Literal(v) => *v,
                AddressTerm::PAddress(target) => self.eval_node(io, *target, context_name)?.as_i64() as u64,
                AddressTerm::Index { index, offset } => {
                    let stride = match offset {
                        IndexOffset::RegisterLength => spec.length as i64,
                        IndexOffset::Literal(v) => *v as i64,
                        IndexOffset::PNode(node) => self.eval_node(io, *node, context_name)?.as_i64(),
                    };
                    let index = self.eval_node(io, *index, context_name)?.as_i64();
                    stride.wrapping_mul(index) as u64
                }
            });
        }
        Ok(addr)
    }

    fn eval_value_source(&self, io: &mut impl RegisterAccess, vs: &ValueSource, context_name: &str) -> Result<Value> {
        match vs {
            ValueSource::LiteralInt(v) => Ok(Value::Int(*v)),
            ValueSource::LiteralFloat(v) => Ok(Value::Float(*v)),
            ValueSource::PValue(target) => self.eval_node(io, *target, context_name),
            ValueSource::Indexed { index, entries, default } => {
                let key = self.eval_node(io, *index, context_name)?.as_i64();
                let source = entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v).unwrap_or(default);
                self.eval_value_source(io, source, context_name)
            }
        }
    }

    fn write_value_source(&self, io: &mut impl RegisterAccess, vs: &ValueSource, value: Value, context_name: &str) -> Result<()> {
        match vs {
            ValueSource::PValue(target) => self.write_node(io, *target, value, context_name),
            _ => Err(GenIcamError::NotWritable(context_name.to_string())),
        }
    }

    fn eval_node(&self, io: &mut impl RegisterAccess, id: NodeId, context_name: &str) -> Result<Value> {
        match &self.nodes[id.index()] {
            Node::Integer(n) | Node::Float(n) => self.eval_value_source(io, &n.value, context_name),
            Node::Boolean(n) => self.eval_value_source(io, &n.value, context_name),
            Node::EnumEntry(e) => Ok(Value::Int(e.value)),
            Node::Enumeration(e) => self.eval_value_source(io, &e.value, context_name),
            Node::Command(c) => self.eval_value_source(io, &c.value, context_name),
            Node::IntReg(spec) => {
                let bytes = self.read_register_bytes(io, id, spec, context_name)?;
                Ok(Value::Int(crate::node::decode_int(&bytes, spec)))
            }
            Node::FloatReg(spec) => {
                let bytes = self.read_register_bytes(io, id, spec, context_name)?;
                Ok(Value::Float(crate::node::decode_float(&bytes, spec.endianness)))
            }
            Node::Converter(conv) => self.eval_converter_read(io, conv, context_name),
            Node::SwissKnife(sk) => {
                let result = self.eval_swiss_knife(io, sk, context_name)?;
                Ok(if sk.is_integer { Value::Int(result.as_i64()) } else { result })
            }
            other @ (Node::StringReg(_) | Node::String(_) | Node::Category(_)) => Err(GenIcamError::TypeMismatch {
                name: context_name.to_string(),
                expected: "numeric node",
                found: other.kind_name(),
            }),
        }
    }

    fn write_node(&self, io: &mut impl RegisterAccess, id: NodeId, value: Value, context_name: &str) -> Result<()> {
        // Clone the small node value up front so the match arms own their data — keeps the
        // recursive calls below straightforward to read, at the cost of a cheap clone.
        let result = match self.nodes[id.index()].clone() {
            Node::Integer(n) | Node::Float(n) => self.write_value_source(io, &n.value, value, context_name),
            Node::Boolean(n) => self.write_value_source(io, &n.value, value, context_name),
            Node::Enumeration(e) => self.write_value_source(io, &e.value, value, context_name),
            Node::IntReg(spec) => {
                let current = if spec.bit_mask.is_some() {
                    self.read_register_bytes(io, id, &spec, context_name)?
                } else {
                    vec![0u8; spec.length as usize]
                };
                let new_bytes = crate::node::encode_int(&current, value.as_i64(), &spec);
                self.write_register_bytes(io, id, &spec, new_bytes, context_name)
            }
            Node::FloatReg(spec) => {
                let new_bytes = crate::node::encode_float(value.as_f64(), spec.length as usize, spec.endianness);
                self.write_register_bytes(io, id, &spec, new_bytes, context_name)
            }
            Node::Converter(conv) => self.eval_converter_write(io, &conv, value, context_name),
            Node::SwissKnife(_)
            | Node::EnumEntry(_)
            | Node::Category(_)
            | Node::Command(_)
            | Node::StringReg(_)
            | Node::String(_) => {
                Err(GenIcamError::NotWritable(context_name.to_string()))
            }
        };
        // Bump this node's own epoch on any successful write, not just direct register writes —
        // invalidators may reference a wrapper feature node (e.g. a plain `Integer` with a
        // `pValue` register behind it) rather than the underlying register itself, and every
        // level of the delegation chain must be able to signal "I changed".
        if result.is_ok() {
            self.bump_epoch(id);
        }
        result
    }

    fn eval_swiss_knife(&self, io: &mut impl RegisterAccess, sk: &SwissKnifeNode, context_name: &str) -> Result<Value> {
        let mut vars = HashMap::new();
        for (name, target) in &sk.variables {
            vars.insert(name.clone(), self.eval_node(io, *target, context_name)?);
        }
        Ok(sk.formula.eval(&vars)?)
    }

    fn eval_converter_read(&self, io: &mut impl RegisterAccess, conv: &ConverterNode, context_name: &str) -> Result<Value> {
        let mut vars = HashMap::new();
        for (name, target) in &conv.variables {
            vars.insert(name.clone(), self.eval_node(io, *target, context_name)?);
        }
        vars.insert("TO".to_string(), self.eval_node(io, conv.value_link, context_name)?);
        let result = conv.formula_from.eval(&vars)?;
        Ok(if conv.is_integer { Value::Int(result.as_i64()) } else { result })
    }

    fn eval_converter_write(&self, io: &mut impl RegisterAccess, conv: &ConverterNode, value: Value, context_name: &str) -> Result<()> {
        let mut vars = HashMap::new();
        for (name, target) in &conv.variables {
            vars.insert(name.clone(), self.eval_node(io, *target, context_name)?);
        }
        vars.insert("FROM".to_string(), value);
        let result = conv.formula_to.eval(&vars)?;
        self.write_node(io, conv.value_link, result, context_name)
    }

    fn read_string_register(&self, io: &mut impl RegisterAccess, id: NodeId, spec: &RegisterAccessSpec, context_name: &str) -> Result<String> {
        let bytes = self.read_register_bytes(io, id, spec, context_name)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    pub fn get_integer(&self, io: &mut impl RegisterAccess, name: &str) -> Result<i64> {
        let id = self.node_id(name)?;
        Ok(self.eval_node(io, id, name)?.as_i64())
    }

    pub fn set_integer(&self, io: &mut impl RegisterAccess, name: &str, value: i64) -> Result<()> {
        let id = self.node_id(name)?;
        self.write_node(io, id, Value::Int(value), name)
    }

    pub fn get_float(&self, io: &mut impl RegisterAccess, name: &str) -> Result<f64> {
        let id = self.node_id(name)?;
        Ok(self.eval_node(io, id, name)?.as_f64())
    }

    pub fn set_float(&self, io: &mut impl RegisterAccess, name: &str, value: f64) -> Result<()> {
        let id = self.node_id(name)?;
        self.write_node(io, id, Value::Float(value), name)
    }

    pub fn get_boolean(&self, io: &mut impl RegisterAccess, name: &str) -> Result<bool> {
        Ok(self.get_integer(io, name)? != 0)
    }

    pub fn set_boolean(&self, io: &mut impl RegisterAccess, name: &str, value: bool) -> Result<()> {
        self.set_integer(io, name, if value { 1 } else { 0 })
    }

    fn read_string_node(&self, io: &mut impl RegisterAccess, id: NodeId, context_name: &str) -> Result<String> {
        match &self.nodes[id.index()] {
            Node::StringReg(spec) => self.read_string_register(io, id, spec, context_name),
            Node::String(StringSource::Literal(text)) => Ok(text.clone()),
            Node::String(StringSource::PValue(target)) => self.read_string_node(io, *target, context_name),
            other => Err(GenIcamError::TypeMismatch {
                name: context_name.to_string(),
                expected: "StringReg",
                found: other.kind_name(),
            }),
        }
    }

    fn write_string_node(&self, io: &mut impl RegisterAccess, id: NodeId, value: &str, context_name: &str) -> Result<()> {
        match &self.nodes[id.index()] {
            Node::StringReg(spec) => {
                let mut bytes = vec![0u8; spec.length as usize];
                let src = value.as_bytes();
                let n = src.len().min(bytes.len());
                bytes[..n].copy_from_slice(&src[..n]);
                self.write_register_bytes(io, id, spec, bytes, context_name)
            }
            Node::String(StringSource::Literal(_)) => Err(GenIcamError::NotWritable(context_name.to_string())),
            Node::String(StringSource::PValue(target)) => {
                self.write_string_node(io, *target, value, context_name)?;
                self.bump_epoch(id);
                Ok(())
            }
            other => Err(GenIcamError::TypeMismatch {
                name: context_name.to_string(),
                expected: "StringReg",
                found: other.kind_name(),
            }),
        }
    }

    pub fn get_string(&self, io: &mut impl RegisterAccess, name: &str) -> Result<String> {
        let id = self.node_id(name)?;
        self.read_string_node(io, id, name)
    }

    pub fn set_string(&self, io: &mut impl RegisterAccess, name: &str, value: &str) -> Result<()> {
        let id = self.node_id(name)?;
        self.write_string_node(io, id, value, name)
    }

    pub fn get_enum_symbolic(&self, io: &mut impl RegisterAccess, name: &str) -> Result<String> {
        let id = self.node_id(name)?;
        let Node::Enumeration(e) = &self.nodes[id.index()] else {
            return Err(GenIcamError::TypeMismatch {
                name: name.to_string(),
                expected: "Enumeration",
                found: self.nodes[id.index()].kind_name(),
            });
        };
        let current = self.eval_value_source(io, &e.value, name)?.as_i64();
        for (entry_name, entry_id) in &e.entries {
            if let Node::EnumEntry(entry) = &self.nodes[entry_id.index()] {
                if entry.value == current {
                    return Ok(entry_name.clone());
                }
            }
        }
        Err(GenIcamError::UnknownEnumEntry {
            name: name.to_string(),
            entry: format!("value={current}"),
        })
    }

    pub fn set_enum_symbolic(&self, io: &mut impl RegisterAccess, name: &str, entry_name: &str) -> Result<()> {
        let id = self.node_id(name)?;
        let (value_source, entry_value) = {
            let Node::Enumeration(e) = &self.nodes[id.index()] else {
                return Err(GenIcamError::TypeMismatch {
                    name: name.to_string(),
                    expected: "Enumeration",
                    found: self.nodes[id.index()].kind_name(),
                });
            };
            let entry_id = e
                .entries
                .iter()
                .find(|(n, _)| n == entry_name)
                .map(|(_, id)| *id)
                .ok_or_else(|| GenIcamError::UnknownEnumEntry {
                    name: name.to_string(),
                    entry: entry_name.to_string(),
                })?;
            let Node::EnumEntry(entry) = &self.nodes[entry_id.index()] else {
                unreachable!("enum entries are always EnumEntry nodes")
            };
            (e.value.clone(), entry.value)
        };
        self.write_value_source(io, &value_source, Value::Int(entry_value), name)
    }

    pub fn execute_command(&self, io: &mut impl RegisterAccess, name: &str) -> Result<()> {
        let id = self.node_id(name)?;
        let (value_source, command_value) = match &self.nodes[id.index()] {
            Node::Command(c) => (c.value.clone(), c.command_value),
            other => {
                return Err(GenIcamError::TypeMismatch {
                    name: name.to_string(),
                    expected: "Command",
                    found: other.kind_name(),
                })
            }
        };
        self.write_value_source(io, &value_source, Value::Int(command_value), name)?;
        // Registers name commands as invalidators (`<pInvalidator>UserSetLoad</pInvalidator>` on
        // the live C6-2040-GigE): executing one must invalidate them, not just the register the
        // command itself writes.
        self.bump_epoch(id);
        Ok(())
    }
}
