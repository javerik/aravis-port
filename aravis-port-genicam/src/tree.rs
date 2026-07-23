use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::access::RegisterAccess;
use crate::dom::{self, XmlDom};
use crate::error::{GenIcamError, Result};
use crate::formula::{Formula, Value};
use crate::node::{
    AddressTerm, BooleanNode, Cachable, CategoryNode, CommandNode, ConverterNode, Endianness,
    EnumEntryNode, EnumerationNode, Node, NodeId, NumericNode, RegisterAccessSpec, Sign,
    SwissKnifeNode, ValueSource,
};

struct CachedValue {
    bytes: Vec<u8>,
    filled_at: u64,
}

/// `(pVariable bindings, Constant name->text, Expression name->text)` collected from a
/// `SwissKnife`/`Converter`-shaped element.
type VariablesConstantsSubExprs = (Vec<(String, NodeId)>, Vec<(String, String)>, Vec<(String, String)>);

/// The parsed GenICam node tree plus register cache/invalidation state. Transport-agnostic:
/// every method that touches the wire takes a `&mut impl RegisterAccess`.
pub struct GenApiTree {
    nodes: Vec<Node>,
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
    "IntReg",
    "MaskedIntReg",
    "FloatReg",
    "Converter",
    "IntConverter",
    "SwissKnife",
    "IntSwissKnife",
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
        if let Some(v) = self.dom.child(idx, "Value") {
            return Ok(ValueSource::LiteralInt(parse_int_literal(&self.dom.get(v).text)?));
        }
        Ok(ValueSource::LiteralInt(0))
    }

    fn value_source_float(&self, idx: usize) -> Result<ValueSource> {
        if let Some(target) = self.resolve_child_ref(idx, "pValue")? {
            return Ok(ValueSource::PValue(target));
        }
        if let Some(v) = self.dom.child(idx, "Value") {
            return Ok(ValueSource::LiteralFloat(parse_float_literal(&self.dom.get(v).text)?));
        }
        Ok(ValueSource::LiteralFloat(0.0))
    }

    fn register_spec(&self, idx: usize) -> Result<RegisterAccessSpec> {
        let mut address_terms = Vec::new();
        for a in self.dom.children_with_tag(idx, "Address") {
            address_terms.push(AddressTerm::Literal(parse_u64_literal(&self.dom.get(a).text)?));
        }
        for a in self.dom.children_with_tag(idx, "pAddress") {
            address_terms.push(AddressTerm::PAddress(self.resolve(&self.dom.get(a).text)?));
        }

        let length = match self.dom.child(idx, "Length") {
            Some(c) => parse_u64_literal(&self.dom.get(c).text)? as u32,
            None => 4,
        };

        let endianness = match self.dom.child(idx, "Endianess").map(|c| self.dom.get(c).text.trim().to_string()) {
            Some(s) if s.eq_ignore_ascii_case("LittleEndian") => Endianness::Little,
            _ => Endianness::Big,
        };

        let sign = match self.dom.child(idx, "Sign").map(|c| self.dom.get(c).text.trim().to_string()) {
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
        let bit_mask = if let Some(bit) = self.dom.child(idx, "Bit") {
            let n = parse_u64_literal(&self.dom.get(bit).text)? as u8;
            let n = if endianness == Endianness::Big { reverse_bit(n) } else { n };
            Some((n, n))
        } else {
            let lsb = self.dom.child(idx, "LSB").map(|c| parse_u64_literal(&self.dom.get(c).text)).transpose()?;
            let msb = self.dom.child(idx, "MSB").map(|c| parse_u64_literal(&self.dom.get(c).text)).transpose()?;
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

        let cachable = match self.dom.child(idx, "Cachable").map(|c| self.dom.get(c).text.trim().to_string()) {
            Some(s) if s.eq_ignore_ascii_case("WriteThrough") => Cachable::WriteThrough,
            Some(s) if s.eq_ignore_ascii_case("WriteAround") => Cachable::WriteAround,
            _ => Cachable::NoCache,
        };

        let writable = !matches!(
            self.dom.child(idx, "AccessMode").map(|c| self.dom.get(c).text.trim().to_string()),
            Some(ref s) if s.eq_ignore_ascii_case("RO")
        );

        let mut invalidators = Vec::new();
        for inv in self.dom.children_with_tag(idx, "pInvalidator") {
            invalidators.push(self.resolve(&self.dom.get(inv).text)?);
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
        })
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
            "Integer" => Node::Integer(NumericNode {
                value: self.value_source_int(idx)?,
                min: None,
                max: None,
            }),
            "Float" => Node::Float(NumericNode {
                value: self.value_source_float(idx)?,
                min: None,
                max: None,
            }),
            "Boolean" => Node::Boolean(BooleanNode {
                value: self.value_source_int(idx)?,
            }),
            "Enumeration" => Node::Enumeration(self.build_enumeration(idx)?),
            "EnumEntry" => Node::EnumEntry(self.build_enum_entry(idx)?),
            "Command" => Node::Command(self.build_command(idx)?),
            "StringReg" => Node::StringReg(self.register_spec(idx)?),
            "IntReg" | "MaskedIntReg" => Node::IntReg(self.register_spec(idx)?),
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
        for (idx, el) in dom.elements.iter().enumerate() {
            if !CONSTRUCTIBLE_TAGS.contains(&el.tag.as_str()) {
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
        };

        // Pass 2: construct the real node value for every reserved slot, now that all names are
        // known. `Builder::build_node` only reads `dom`/`names`/`dom_to_node`, never
        // `builder.nodes` — that field exists solely to size `built_nodes` below.
        let mut built_nodes: Vec<Node> = (0..n).map(|_| Node::Category(CategoryNode::default())).collect();
        for (&idx, &id) in &builder.dom_to_node {
            built_nodes[id.index()] = builder.build_node(idx)?;
        }

        let n = built_nodes.len();
        Ok(Self {
            nodes: built_nodes,
            names: builder.names,
            cache: (0..n).map(|_| RefCell::new(None)).collect(),
            epoch: (0..n).map(|_| Cell::new(0)).collect(),
            global_tick: Cell::new(0),
            caching_enabled: Cell::new(true),
        })
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
        if spec.cachable != Cachable::NoCache && self.caching_enabled.get() {
            let cached = self.cache[id.index()].borrow();
            if let Some(c) = cached.as_ref() {
                let valid = spec
                    .invalidators
                    .iter()
                    .all(|inv| self.epoch[inv.index()].get() <= c.filled_at);
                if valid {
                    return Ok(c.bytes.clone());
                }
            }
        }
        let address = self.resolve_address(io, spec, context_name)?;
        let bytes = io
            .read_memory(address, spec.length as usize)
            .map_err(|e| GenIcamError::Io(e.to_string()))?;
        if spec.cachable != Cachable::NoCache {
            *self.cache[id.index()].borrow_mut() = Some(CachedValue {
                bytes: bytes.clone(),
                filled_at: self.global_tick.get(),
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
        if !spec.writable {
            return Err(GenIcamError::NotWritable(context_name.to_string()));
        }
        let address = self.resolve_address(io, spec, context_name)?;
        io.write_memory(address, &new_bytes).map_err(|e| GenIcamError::Io(e.to_string()))?;
        self.bump_epoch(id);
        *self.cache[id.index()].borrow_mut() = if spec.cachable == Cachable::WriteThrough {
            Some(CachedValue {
                bytes: new_bytes,
                filled_at: self.global_tick.get(),
            })
        } else {
            None
        };
        Ok(())
    }

    fn resolve_address(&self, io: &mut impl RegisterAccess, spec: &RegisterAccessSpec, context_name: &str) -> Result<u64> {
        let mut addr = 0u64;
        for term in &spec.address_terms {
            addr += match term {
                AddressTerm::Literal(v) => *v,
                AddressTerm::PAddress(target) => self.eval_node(io, *target, context_name)?.as_i64() as u64,
            };
        }
        Ok(addr)
    }

    fn eval_value_source(&self, io: &mut impl RegisterAccess, vs: &ValueSource, context_name: &str) -> Result<Value> {
        match vs {
            ValueSource::LiteralInt(v) => Ok(Value::Int(*v)),
            ValueSource::LiteralFloat(v) => Ok(Value::Float(*v)),
            ValueSource::PValue(target) => self.eval_node(io, *target, context_name),
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
            other @ (Node::StringReg(_) | Node::Category(_)) => Err(GenIcamError::TypeMismatch {
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
            Node::SwissKnife(_) | Node::EnumEntry(_) | Node::Category(_) | Node::Command(_) | Node::StringReg(_) => {
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

    pub fn get_string(&self, io: &mut impl RegisterAccess, name: &str) -> Result<String> {
        let id = self.node_id(name)?;
        match &self.nodes[id.index()] {
            Node::StringReg(spec) => self.read_string_register(io, id, spec, name),
            other => Err(GenIcamError::TypeMismatch {
                name: name.to_string(),
                expected: "StringReg",
                found: other.kind_name(),
            }),
        }
    }

    pub fn set_string(&self, io: &mut impl RegisterAccess, name: &str, value: &str) -> Result<()> {
        let id = self.node_id(name)?;
        let spec = match &self.nodes[id.index()] {
            Node::StringReg(spec) => spec.clone(),
            other => {
                return Err(GenIcamError::TypeMismatch {
                    name: name.to_string(),
                    expected: "StringReg",
                    found: other.kind_name(),
                })
            }
        };
        let mut bytes = vec![0u8; spec.length as usize];
        let src = value.as_bytes();
        let n = src.len().min(bytes.len());
        bytes[..n].copy_from_slice(&src[..n]);
        self.write_register_bytes(io, id, &spec, bytes, name)
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
        self.write_value_source(io, &value_source, Value::Int(command_value), name)
    }
}
