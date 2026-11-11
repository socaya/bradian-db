//! Recursive-descent parser and interpreter for the DHIS2 Program Indicator / Indicator
//! expression subset supported in v1.
//!
//! Grammar (EBNF):
//!   expr    = add
//!   add     = mul (("+" | "-") mul)*
//!   mul     = unary (("*" | "/") unary)*
//!   unary   = ("+" | "-")? primary
//!   primary = NUMBER | REF | "(" expr ")"
//!   REF     = "#{" UID ( "." UID )? "}"
//!
//! Numbers accept optional leading sign (via unary), optional decimal point, and no scientific
//! notation (DHIS2 indicator expressions do not use it).
//!
//! Unknown function calls (identified by `identifier(` tokens) and SQL-like constructs are
//! rejected with `UnsupportedFeature`.

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExpressionAst {
    Number(f64),
    DataElement {
        de: String,
        coc: Option<String>,
    },
    BinaryOp {
        op: BinOp,
        lhs: Box<ExpressionAst>,
        rhs: Box<ExpressionAst>,
    },
    Neg(Box<ExpressionAst>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

pub fn compile(src: &str) -> DapResult<ExpressionAst> {
    let tokens = tokenize(src)?;
    let mut p = Parser { tokens, pos: 0, src };
    let ast = p.parse_expr()?;
    if p.pos != p.tokens.len() {
        return Err(DapError::invalid_value(format!(
            "unexpected trailing input at offset {}",
            p.tokens[p.pos].offset
        )));
    }
    Ok(ast)
}

/// Evaluate the expression with a resolver that returns the value of a `#{DE.COC}` reference.
/// Returns `None` if the resolver returns `None` for any referenced value (NULL propagation).
pub fn evaluate(
    ast: &ExpressionAst,
    resolver: &mut dyn FnMut(&str, Option<&str>) -> Option<f64>,
) -> DapResult<Option<f64>> {
    Ok(match ast {
        ExpressionAst::Number(n) => Some(*n),
        ExpressionAst::DataElement { de, coc } => resolver(de, coc.as_deref()),
        ExpressionAst::Neg(inner) => evaluate(inner, resolver)?.map(|v| -v),
        ExpressionAst::BinaryOp { op, lhs, rhs } => {
            let l = match evaluate(lhs, resolver)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let r = match evaluate(rhs, resolver)? {
                Some(v) => v,
                None => return Ok(None),
            };
            match op {
                BinOp::Add => Some(l + r),
                BinOp::Sub => Some(l - r),
                BinOp::Mul => Some(l * r),
                BinOp::Div => {
                    if r == 0.0 {
                        // DHIS2 convention: divide by zero → NULL, not error.
                        None
                    } else {
                        Some(l / r)
                    }
                }
            }
        }
    })
}

// ------------------------------- Lexer -------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Number(f64),
    Ref(String, Option<String>),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    offset: usize,
}

fn tokenize(src: &str) -> DapResult<Vec<Token>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            '+' => {
                out.push(Token { tok: Tok::Plus, offset: start });
                i += 1;
            }
            '-' => {
                out.push(Token { tok: Tok::Minus, offset: start });
                i += 1;
            }
            '*' => {
                out.push(Token { tok: Tok::Star, offset: start });
                i += 1;
            }
            '/' => {
                out.push(Token { tok: Tok::Slash, offset: start });
                i += 1;
            }
            '(' => {
                out.push(Token { tok: Tok::LParen, offset: start });
                i += 1;
            }
            ')' => {
                out.push(Token { tok: Tok::RParen, offset: start });
                i += 1;
            }
            '0'..='9' | '.' => {
                let mut end = i;
                while end < bytes.len()
                    && (bytes[end].is_ascii_digit() || bytes[end] == b'.')
                {
                    end += 1;
                }
                let s = &src[i..end];
                let value: f64 = s.parse().map_err(|e: std::num::ParseFloatError| {
                    DapError::invalid_value(format!("numeric literal '{s}': {e}"))
                })?;
                out.push(Token {
                    tok: Tok::Number(value),
                    offset: start,
                });
                i = end;
            }
            '#' => {
                if i + 1 >= bytes.len() || bytes[i + 1] != b'{' {
                    return Err(DapError::invalid_value(format!(
                        "expected '#{{' at offset {i}"
                    )));
                }
                let brace_start = i + 2;
                let close = src[brace_start..]
                    .find('}')
                    .ok_or_else(|| DapError::invalid_value("unterminated #{...} reference"))?;
                let inner = &src[brace_start..brace_start + close];
                let (de, coc) = match inner.split_once('.') {
                    Some((d, c)) => (d.trim().to_string(), Some(c.trim().to_string())),
                    None => (inner.trim().to_string(), None),
                };
                if de.is_empty() {
                    return Err(DapError::invalid_value("empty data-element UID in reference"));
                }
                out.push(Token {
                    tok: Tok::Ref(de, coc),
                    offset: start,
                });
                i = brace_start + close + 1;
            }
            // Function call? Identifier followed by '(' — reject explicitly.
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut end = i;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                let name = &src[i..end];
                let next = bytes.get(end).copied();
                if next == Some(b'(') {
                    return Err(DapError::UnsupportedFeature(format!(
                        "function '{name}' is not in the v1 expression grammar; \
                         compile a program indicator that uses it only after the function \
                         matrix lands"
                    )));
                }
                return Err(DapError::invalid_value(format!(
                    "unexpected identifier '{name}' at offset {i}"
                )));
            }
            other => {
                return Err(DapError::invalid_value(format!(
                    "unexpected character {other:?} at offset {i}"
                )));
            }
        }
    }
    Ok(out)
}

// ------------------------------- Parser -------------------------------

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    src: &'a str,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn bump(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> DapResult<ExpressionAst> {
        self.parse_add()
    }

    fn parse_add(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_mul()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Plus) => Some(BinOp::Add),
            Some(Tok::Minus) => Some(BinOp::Sub),
            _ => None,
        } {
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = ExpressionAst::BinaryOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_unary()?;
        while let Some(op) = match self.peek() {
            Some(Tok::Star) => Some(BinOp::Mul),
            Some(Tok::Slash) => Some(BinOp::Div),
            _ => None,
        } {
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = ExpressionAst::BinaryOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> DapResult<ExpressionAst> {
        match self.peek() {
            Some(Tok::Plus) => {
                self.bump();
                self.parse_unary()
            }
            Some(Tok::Minus) => {
                self.bump();
                let inner = self.parse_unary()?;
                Ok(ExpressionAst::Neg(Box::new(inner)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> DapResult<ExpressionAst> {
        match self.bump() {
            Some(t) => match &t.tok {
                Tok::Number(n) => Ok(ExpressionAst::Number(*n)),
                Tok::Ref(de, coc) => Ok(ExpressionAst::DataElement {
                    de: de.clone(),
                    coc: coc.clone(),
                }),
                Tok::LParen => {
                    let inner = self.parse_expr()?;
                    let src = self.src.to_string();
                    match self.bump() {
                        Some(Token { tok: Tok::RParen, .. }) => Ok(inner),
                        other => Err(DapError::invalid_value(format!(
                            "expected ')', got {other:?} in '{src}'"
                        ))),
                    }
                }
                other => Err(DapError::invalid_value(format!(
                    "unexpected token {other:?} at offset {}",
                    t.offset
                ))),
            },
            None => Err(DapError::invalid_value("unexpected end of expression")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str, values: &[(&str, f64)]) -> Option<f64> {
        let ast = compile(src).unwrap();
        let map: ahash::AHashMap<String, f64> =
            values.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        let mut lookup = |de: &str, _coc: Option<&str>| map.get(de).copied();
        evaluate(&ast, &mut lookup).unwrap()
    }

    #[test]
    fn simple_arithmetic() {
        assert_eq!(eval("1 + 2 * 3", &[]), Some(7.0));
        assert_eq!(eval("(1 + 2) * 3", &[]), Some(9.0));
        assert_eq!(eval("-4 + 1", &[]), Some(-3.0));
    }

    #[test]
    fn data_element_ref() {
        assert_eq!(
            eval(
                "#{deAAAAAAAAA} + #{deBBBBBBBBB}",
                &[("deAAAAAAAAA", 10.0), ("deBBBBBBBBB", 5.0)],
            ),
            Some(15.0)
        );
    }

    #[test]
    fn null_propagation() {
        // deMISSINGXXX has no value -> entire expression is NULL.
        assert_eq!(
            eval(
                "#{deAAAAAAAAA} + #{deMISSING00}",
                &[("deAAAAAAAAA", 1.0)]
            ),
            None
        );
    }

    #[test]
    fn divide_by_zero_yields_null() {
        assert_eq!(eval("1 / (2 - 2)", &[]), None);
    }

    #[test]
    fn de_coc_ref_parsed() {
        let ast = compile("#{deAAAAAAAAA.cocAAAAAAAA}").unwrap();
        if let ExpressionAst::DataElement { de, coc } = ast {
            assert_eq!(de, "deAAAAAAAAA");
            assert_eq!(coc.as_deref(), Some("cocAAAAAAAA"));
        } else {
            panic!("wrong AST shape");
        }
    }

    #[test]
    fn function_calls_rejected_explicitly() {
        let err = compile("sum(#{de1})").expect_err("should reject function");
        let msg = err.to_string();
        assert!(msg.contains("function 'sum'") || msg.contains("not in the v1"), "got: {msg}");
    }

    #[test]
    fn unexpected_identifier_rejected() {
        let err = compile("abc + 1").expect_err("should reject bare identifier");
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn unmatched_paren_rejected() {
        assert!(compile("(1 + 2").is_err());
        assert!(compile("1 + 2)").is_err());
    }
}
