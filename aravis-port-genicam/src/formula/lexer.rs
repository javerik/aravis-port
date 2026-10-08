use crate::error::FormulaError;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Int(i64),
    Float(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Pow,
    Tilde,
    Amp,
    Pipe,
    Caret,
    AndAnd,
    OrOr,
    Shl,
    Shr,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Question,
    Colon,
    Comma,
    LParen,
    RParen,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, FormulaError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '0' && matches!(chars.get(i + 1), Some('x') | Some('X')) {
            let start = i + 2;
            let mut j = start;
            while j < chars.len() && chars[j].is_ascii_hexdigit() {
                j += 1;
            }
            let text: String = chars[start..j].iter().collect();
            let value = i64::from_str_radix(&text, 16).map_err(|_| FormulaError::Expected {
                expected: "hex literal",
                found: text.clone(),
            })?;
            tokens.push(Token::Int(value));
            i = j;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()))
        {
            let start = i;
            let mut is_float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' {
                    is_float = true;
                }
                i += 1;
            }
            // Scientific notation, e.g. 1e-3.
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    is_float = true;
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            if is_float {
                tokens.push(Token::Float(text.parse().map_err(|_| {
                    FormulaError::Expected {
                        expected: "number",
                        found: text.clone(),
                    }
                })?));
            } else {
                tokens.push(Token::Int(text.parse().map_err(|_| {
                    FormulaError::Expected {
                        expected: "number",
                        found: text.clone(),
                    }
                })?));
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        macro_rules! two_char {
            ($second:expr, $two:expr, $one:expr) => {{
                if chars.get(i + 1) == Some(&$second) {
                    i += 2;
                    tokens.push($two);
                } else {
                    i += 1;
                    tokens.push($one);
                }
            }};
        }
        match c {
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => two_char!('*', Token::Pow, Token::Star),
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
            }
            '~' => {
                tokens.push(Token::Tilde);
                i += 1;
            }
            '&' => two_char!('&', Token::AndAnd, Token::Amp),
            '|' => two_char!('|', Token::OrOr, Token::Pipe),
            '^' => {
                tokens.push(Token::Caret);
                i += 1;
            }
            '<' => {
                if chars.get(i + 1) == Some(&'=') {
                    tokens.push(Token::Le);
                    i += 2;
                } else if chars.get(i + 1) == Some(&'>') {
                    tokens.push(Token::Ne);
                    i += 2;
                } else if chars.get(i + 1) == Some(&'<') {
                    tokens.push(Token::Shl);
                    i += 2;
                } else {
                    tokens.push(Token::Lt);
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') {
                    tokens.push(Token::Ge);
                    i += 2;
                } else if chars.get(i + 1) == Some(&'>') {
                    tokens.push(Token::Shr);
                    i += 2;
                } else {
                    tokens.push(Token::Gt);
                    i += 1;
                }
            }
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
            }
            '?' => {
                tokens.push(Token::Question);
                i += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            other => return Err(FormulaError::UnexpectedChar(other, i)),
        }
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_ternary_and_comparison() {
        let tokens = tokenize("X=648 ? A : B").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Ident("X".into()),
                Token::Eq,
                Token::Int(648),
                Token::Question,
                Token::Ident("A".into()),
                Token::Colon,
                Token::Ident("B".into()),
            ]
        );
    }

    #[test]
    fn tokenizes_multi_char_operators() {
        let tokens = tokenize("A**2 <> B && C || D << 1 >> 2 <= 3 >= 4").unwrap();
        assert!(tokens.contains(&Token::Pow));
        assert!(tokens.contains(&Token::Ne));
        assert!(tokens.contains(&Token::AndAnd));
        assert!(tokens.contains(&Token::OrOr));
        assert!(tokens.contains(&Token::Shl));
        assert!(tokens.contains(&Token::Shr));
        assert!(tokens.contains(&Token::Le));
        assert!(tokens.contains(&Token::Ge));
    }

    #[test]
    fn tokenizes_hex_literals() {
        let tokens = tokenize("SEL * 0x10").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Ident("SEL".into()), Token::Star, Token::Int(0x10)]
        );
    }

    #[test]
    fn tokenizes_float_literals() {
        let tokens = tokenize("1.5 + .5 + 1e-3").unwrap();
        assert_eq!(tokens[0], Token::Float(1.5));
        assert_eq!(tokens[2], Token::Float(0.5));
        assert_eq!(tokens[4], Token::Float(1e-3));
    }
}
