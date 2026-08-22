//! The Pratt parser for expressions.
//!
//! Left and right binding powers differ by one; the right side is the larger of
//! the two for a left-associative operator. Keeping the table in one place stops
//! precedence from being spread across the shape of the code.

use sic_core::Span;

use crate::ast::*;
use crate::token::{Keyword, TokenKind};

use super::Parser;

/// Binding power of the prefix operators: tighter than any binary operator,
/// looser than the postfix ones.
const PREFIX_BP: u8 = 13;
/// Binding power of the postfix operators (calls and field access).
const POSTFIX_BP: u8 = 15;

/// The (operator, left bp, right bp) of a binary operator. All are left-associative.
fn infix_bp(kind: &TokenKind) -> Option<(BinOp, u8, u8)> {
    let (op, lbp) = match kind {
        TokenKind::PipePipe => (BinOp::Or, 1),
        TokenKind::AmpAmp => (BinOp::And, 3),
        TokenKind::EqEq => (BinOp::Eq, 5),
        TokenKind::BangEq => (BinOp::Ne, 5),
        TokenKind::Lt => (BinOp::Lt, 7),
        TokenKind::Le => (BinOp::Le, 7),
        TokenKind::Gt => (BinOp::Gt, 7),
        TokenKind::Ge => (BinOp::Ge, 7),
        TokenKind::Plus => (BinOp::Add, 9),
        TokenKind::Minus => (BinOp::Sub, 9),
        TokenKind::Star => (BinOp::Mul, 11),
        TokenKind::Slash => (BinOp::Div, 11),
        TokenKind::Percent => (BinOp::Rem, 11),
        _ => return None,
    };
    Some((op, lbp, lbp + 1))
}

impl Parser {
    pub(super) fn parse_expr(&mut self) -> Expr {
        self.expr_bp(0)
    }

    /// Stops and returns the subexpression built so far as soon as it meets an
    /// operator whose binding power is below `min_bp`.
    fn expr_bp(&mut self, min_bp: u8) -> Expr {
        let mut lhs = self.parse_prefix();

        loop {
            // Postfix binds tightest, so it is handled first.
            match self.peek() {
                TokenKind::LParen if POSTFIX_BP > min_bp => {
                    lhs = self.parse_call(lhs);
                    continue;
                }
                TokenKind::Dot if POSTFIX_BP > min_bp => {
                    lhs = self.parse_field(lhs);
                    continue;
                }
                _ => {}
            }

            let Some((op, lbp, rbp)) = infix_bp(self.peek()) else {
                return lhs;
            };
            if lbp < min_bp {
                return lhs;
            }
            self.bump(); // the operator
            let rhs = self.expr_bp(rbp);
            let span = lhs.span.to(rhs.span);
            let id = self.id();
            lhs = Expr {
                id,
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
    }

    fn parse_prefix(&mut self) -> Expr {
        let start = self.span().lo;
        match self.peek().clone() {
            TokenKind::Minus | TokenKind::Bang => {
                let op = if matches!(self.peek(), TokenKind::Minus) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                };
                self.bump();
                let operand = self.expr_bp(PREFIX_BP);
                let span = Span::new(start, operand.span.hi);
                let id = self.id();
                Expr {
                    id,
                    kind: ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            TokenKind::Int(v) => self.literal(ExprKind::Int(v)),
            TokenKind::Float(v) => self.literal(ExprKind::Float(v)),
            TokenKind::Str(s) => self.literal(ExprKind::Str(s)),
            TokenKind::Kw(Keyword::True) => self.literal(ExprKind::Bool(true)),
            TokenKind::Kw(Keyword::False) => self.literal(ExprKind::Bool(false)),
            TokenKind::Kw(Keyword::Null) => self.literal(ExprKind::Null),
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                let id = self.id();
                Expr {
                    id,
                    kind: ExprKind::Path(Ident { name, span }),
                    span,
                }
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.expr_bp(0);
                self.expect(&TokenKind::RParen, "to close a parenthesized expression");
                // The parentheses leave no trace in the AST; only the span grows.
                Expr {
                    span: Span::new(start, self.prev_end()),
                    ..inner
                }
            }
            TokenKind::Kw(kw) if kw.is_reserved() => {
                let span = self.bump().span;
                self.error(
                    "E0210",
                    format!("`{}` is reserved for future use", kw.text()),
                    span,
                    "not available in v0.1",
                );
                self.error_expr(span)
            }
            other => {
                // Nothing is consumed here. Consuming the token would eat the
                // `)` or `;` in an input like `1 +`, leaving nothing for the
                // caller to recover on.
                let span = Span::empty(start);
                self.error(
                    "E0204",
                    "expected an expression",
                    span,
                    format!("found {}", other.describe()),
                );
                self.error_expr(span)
            }
        }
    }

    fn literal(&mut self, kind: ExprKind) -> Expr {
        let span = self.bump().span;
        let id = self.id();
        Expr { id, kind, span }
    }

    fn parse_call(&mut self, callee: Expr) -> Expr {
        self.bump(); // `(`
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            let before = self.pos;
            args.push(self.expr_bp(0));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::RParen, "after the argument list");
        let span = Span::new(callee.span.lo, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
            span,
        }
    }

    fn parse_field(&mut self, base: Expr) -> Expr {
        self.bump(); // `.`
        let name = self.expect_ident("a field name");
        let span = Span::new(base.span.lo, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Field {
                base: Box::new(base),
                name,
            },
            span,
        }
    }
}
