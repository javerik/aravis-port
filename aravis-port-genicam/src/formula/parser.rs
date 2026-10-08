use std::collections::HashMap;

use crate::error::FormulaError;

use super::eval::Value;
use super::lexer::{tokenize, Token};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Plus,
    Neg,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    And,
    Or,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
}

/// Unary functions. All except `Sgn`/`Neg` always compute in `f64` regardless of argument type,
/// matching the reference evaluator's `double_only` set — this is a deliberate, documented
/// simplification: we don't model a separate "integer evaluation mode" at all, always allowing
/// mixed int/float arithmetic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Func {
    Sin,
    Cos,
    Sgn,
    Neg,
    Atan,
    Tan,
    Abs,
    Exp,
    Ln,
    Lg,
    Sqrt,
    Trunc,
    Round,
    Floor,
    Ceil,
    Asin,
    Acos,
}

impl Func {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "SIN" => Func::Sin,
            "COS" => Func::Cos,
            "SGN" => Func::Sgn,
            "NEG" => Func::Neg,
            "ATAN" => Func::Atan,
            "TAN" => Func::Tan,
            "ABS" => Func::Abs,
            "EXP" => Func::Exp,
            "LN" => Func::Ln,
            "LG" => Func::Lg,
            "SQRT" => Func::Sqrt,
            "TRUNC" => Func::Trunc,
            "ROUND" => Func::Round,
            "FLOOR" => Func::Floor,
            "CEIL" => Func::Ceil,
            "ASIN" => Func::Asin,
            "ACOS" => Func::Acos,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Func::Sin => "SIN",
            Func::Cos => "COS",
            Func::Sgn => "SGN",
            Func::Neg => "NEG",
            Func::Atan => "ATAN",
            Func::Tan => "TAN",
            Func::Abs => "ABS",
            Func::Exp => "EXP",
            Func::Ln => "LN",
            Func::Lg => "LG",
            Func::Sqrt => "SQRT",
            Func::Trunc => "TRUNC",
            Func::Round => "ROUND",
            Func::Floor => "FLOOR",
            Func::Ceil => "CEIL",
            Func::Asin => "ASIN",
            Func::Acos => "ACOS",
        }
    }

    /// `(min_args, max_args)`. Only `ROUND` is variadic (1 or 2 args).
    fn arity(self) -> (usize, usize) {
        match self {
            Func::Round => (1, 2),
            _ => (1, 1),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Const(Value),
    Var(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

fn binop_info(tok: &Token) -> Option<(BinOp, u8, bool)> {
    use Token::*;
    Some(match tok {
        OrOr => (BinOp::Or, 10, false),
        AndAnd => (BinOp::And, 20, false),
        Pipe => (BinOp::BitOr, 40, false),
        Caret => (BinOp::BitXor, 50, false),
        Amp => (BinOp::BitAnd, 60, false),
        Eq => (BinOp::Eq, 70, false),
        Ne => (BinOp::Ne, 70, false),
        Le => (BinOp::Le, 80, false),
        Ge => (BinOp::Ge, 80, false),
        Lt => (BinOp::Lt, 80, false),
        Gt => (BinOp::Gt, 80, false),
        Shl => (BinOp::Shl, 90, false),
        Shr => (BinOp::Shr, 90, false),
        Plus => (BinOp::Add, 100, false),
        Minus => (BinOp::Sub, 100, false),
        Percent => (BinOp::Mod, 110, false),
        Slash => (BinOp::Div, 110, false),
        Star => (BinOp::Mul, 110, false),
        Pow => (BinOp::Pow, 120, true),
        _ => return None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    constants: HashMap<String, Value>,
    sub_expressions: HashMap<String, Vec<Token>>,
    /// `true` while parsing inside an already-inlined sub-expression: a reference to *any*
    /// registered sub-expression name here is an error, not silently treated as a plain
    /// variable, per the spec's "no recursive sub-expression nesting" rule.
    in_sub_expression: bool,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect(&mut self, expected: Token) -> Result<(), FormulaError> {
        match self.bump() {
            Some(t) if t == expected => Ok(()),
            Some(t) => Err(FormulaError::Expected {
                expected: "expected token",
                found: format!("{t:?}"),
            }),
            None => Err(FormulaError::UnexpectedEnd),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, FormulaError> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<Expr, FormulaError> {
        let cond = self.parse_binary(10)?;
        if self.peek() == Some(&Token::Question) {
            self.bump();
            let then_branch = self.parse_ternary()?;
            self.expect(Token::Colon)?;
            let else_branch = self.parse_ternary()?;
            Ok(Expr::Ternary(
                Box::new(cond),
                Box::new(then_branch),
                Box::new(else_branch),
            ))
        } else {
            Ok(cond)
        }
    }

    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, FormulaError> {
        let mut lhs = self.parse_unary()?;
        while let Some((op, prec, right_assoc)) = self.peek().and_then(binop_info) {
            if prec < min_prec {
                break;
            }
            self.bump();
            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.parse_binary(next_min)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, FormulaError> {
        match self.peek() {
            Some(Token::Minus) => {
                self.bump();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.parse_unary()?)))
            }
            Some(Token::Plus) => {
                self.bump();
                Ok(Expr::Unary(UnOp::Plus, Box::new(self.parse_unary()?)))
            }
            Some(Token::Tilde) => {
                self.bump();
                Ok(Expr::Unary(UnOp::BitNot, Box::new(self.parse_unary()?)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, FormulaError> {
        match self.bump() {
            Some(Token::Int(v)) => Ok(Expr::Const(Value::Int(v))),
            Some(Token::Float(v)) => Ok(Expr::Const(Value::Float(v))),
            Some(Token::LParen) => {
                let e = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(e)
            }
            Some(Token::Ident(name)) => self.parse_ident(name),
            Some(other) => Err(FormulaError::Expected {
                expected: "expression",
                found: format!("{other:?}"),
            }),
            None => Err(FormulaError::UnexpectedEnd),
        }
    }

    fn parse_ident(&mut self, name: String) -> Result<Expr, FormulaError> {
        if self.peek() == Some(&Token::LParen) {
            if let Some(func) = Func::from_name(&name) {
                return self.parse_call(func);
            }
        }
        if let Some(v) = self.constants.get(&name) {
            return Ok(Expr::Const(*v));
        }
        if let Some(tokens) = self.sub_expressions.get(&name) {
            if self.in_sub_expression {
                return Err(FormulaError::RecursiveSubExpression(name));
            }
            let mut nested = Parser {
                tokens: tokens.clone(),
                pos: 0,
                constants: self.constants.clone(),
                sub_expressions: self.sub_expressions.clone(),
                in_sub_expression: true,
            };
            let expr = nested.parse_expr()?;
            if nested.pos != nested.tokens.len() {
                return Err(FormulaError::TrailingInput(format!(
                    "{:?}",
                    nested.tokens[nested.pos]
                )));
            }
            return Ok(expr);
        }
        Ok(Expr::Var(name))
    }

    fn parse_call(&mut self, func: Func) -> Result<Expr, FormulaError> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        if self.peek() != Some(&Token::RParen) {
            args.push(self.parse_expr()?);
            while self.peek() == Some(&Token::Comma) {
                self.bump();
                args.push(self.parse_expr()?);
            }
        }
        self.expect(Token::RParen)?;
        let (min, max) = func.arity();
        if args.len() < min || args.len() > max {
            let expected: &'static str = if min == max {
                match min {
                    1 => "1 argument",
                    _ => "arguments",
                }
            } else {
                "1 or 2 arguments"
            };
            return Err(FormulaError::WrongArgCount {
                name: func.name().to_string(),
                expected,
                got: args.len(),
            });
        }
        Ok(Expr::Call(func, args))
    }
}

/// A parsed, ready-to-evaluate formula. `Constant`/`Expression` (named sub-formula) substitution
/// happens once at parse time, not per evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    pub(super) expr: Expr,
}

fn parse_constant_value(text: &str) -> Result<Value, FormulaError> {
    let trimmed = text.trim();
    if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
        if let Ok(f) = trimmed.parse::<f64>() {
            return Ok(Value::Float(f));
        }
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Ok(Value::Int(i));
    }
    trimmed
        .parse::<f64>()
        .map(Value::Float)
        .map_err(|_| FormulaError::Expected {
            expected: "numeric constant",
            found: text.to_string(),
        })
}

impl Formula {
    pub fn parse(
        text: &str,
        constants: &[(String, String)],
        sub_expressions: &[(String, String)],
    ) -> Result<Self, FormulaError> {
        let mut const_map = HashMap::new();
        for (name, value) in constants {
            const_map.insert(name.clone(), parse_constant_value(value)?);
        }
        let mut sub_map = HashMap::new();
        for (name, text) in sub_expressions {
            sub_map.insert(name.clone(), tokenize(text)?);
        }

        let tokens = tokenize(text)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            constants: const_map,
            sub_expressions: sub_map,
            in_sub_expression: false,
        };
        let expr = parser.parse_expr()?;
        if parser.pos != parser.tokens.len() {
            return Err(FormulaError::TrailingInput(format!(
                "{:?}",
                parser.tokens[parser.pos]
            )));
        }
        Ok(Self { expr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Formula {
        Formula::parse(text, &[], &[]).unwrap()
    }

    #[test]
    fn precedence_multiplication_over_addition() {
        let f = parse("2 + 3 * 4");
        let v = crate::formula::eval_expr(&f.expr, &HashMap::new()).unwrap();
        assert_eq!(v, Value::Int(14));
    }

    #[test]
    fn precedence_power_right_associative() {
        // 2 ** 3 ** 2 == 2 ** (3 ** 2) == 2**9 == 512, not (2**3)**2 == 64.
        let f = parse("2 ** 3 ** 2");
        let v = crate::formula::eval_expr(&f.expr, &HashMap::new()).unwrap();
        assert_eq!(v, Value::Float(512.0));
    }

    #[test]
    fn ternary_is_right_associative_and_lower_precedence_than_or() {
        let f = parse("1 || 0 ? 10 : 20");
        let v = crate::formula::eval_expr(&f.expr, &HashMap::new()).unwrap();
        assert_eq!(v, Value::Int(10));
    }

    #[test]
    fn parens_override_precedence() {
        let f = parse("(2 + 3) * 4");
        let v = crate::formula::eval_expr(&f.expr, &HashMap::new()).unwrap();
        assert_eq!(v, Value::Int(20));
    }

    #[test]
    fn round_accepts_one_or_two_args() {
        assert!(Formula::parse("ROUND(1.5)", &[], &[]).is_ok());
        assert!(Formula::parse("ROUND(1.5, 2)", &[], &[]).is_ok());
        assert!(Formula::parse("ROUND(1.5, 2, 3)", &[], &[]).is_err());
    }

    #[test]
    fn sin_rejects_wrong_arg_count() {
        assert!(Formula::parse("SIN(1, 2)", &[], &[]).is_err());
        assert!(Formula::parse("SIN()", &[], &[]).is_err());
    }

    #[test]
    fn recursive_sub_expression_reference_is_rejected() {
        let result = Formula::parse(
            "A + 1",
            &[],
            &[
                ("A".to_string(), "B".to_string()),
                ("B".to_string(), "A".to_string()),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn constants_and_sub_expressions_are_inlined() {
        let f = Formula::parse(
            "X=CMV300 ? 1 : 2",
            &[("CMV300".to_string(), "1".to_string())],
            &[],
        )
        .unwrap();
        let mut vars = HashMap::new();
        vars.insert("X".to_string(), Value::Int(1));
        let v = crate::formula::eval_expr(&f.expr, &vars).unwrap();
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn trailing_input_is_rejected() {
        assert!(Formula::parse("1 + 1 2", &[], &[]).is_err());
    }
}
