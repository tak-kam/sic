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

    /// Parses the condition of an `if`, where a `{` starts the body rather than
    /// a struct literal.
    pub(super) fn parse_condition(&mut self) -> Expr {
        self.no_struct += 1;
        let expr = self.expr_bp(0);
        self.no_struct -= 1;
        expr
    }

    /// Parses inside a delimiter, where a struct literal is unambiguous again.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::take(&mut self.no_struct);
        let out = parse(self);
        self.no_struct = saved;
        out
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
                TokenKind::LBracket if POSTFIX_BP > min_bp => {
                    lhs = self.parse_index(lhs);
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
            TokenKind::Kw(Keyword::Spawn) => {
                self.bump();
                self.parse_spawn(start)
            }
            TokenKind::Kw(Keyword::Await) => {
                self.bump();
                let task = self.expr_bp(PREFIX_BP);
                let span = Span::new(start, task.span.hi);
                let id = self.id();
                Expr {
                    id,
                    kind: ExprKind::Await {
                        task: Box::new(task),
                    },
                    span,
                }
            }
            TokenKind::Kw(Keyword::True) => self.literal(ExprKind::Bool(true)),
            TokenKind::Kw(Keyword::False) => self.literal(ExprKind::Bool(false)),
            TokenKind::Kw(Keyword::Null) => self.literal(ExprKind::Null),
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                let name = Ident { name, span };
                if self.at(&TokenKind::LBrace) && self.no_struct == 0 {
                    return self.parse_struct_literal(name);
                }
                let id = self.id();
                Expr {
                    id,
                    kind: ExprKind::Path(name),
                    span,
                }
            }
            TokenKind::LBracket => {
                self.bump();
                let elements = self.nested(|p| {
                    let mut elements = Vec::new();
                    while !p.at(&TokenKind::RBracket) && !p.at_eof() {
                        let before = p.pos;
                        elements.push(p.expr_bp(0));
                        if !p.eat(&TokenKind::Comma) {
                            break;
                        }
                        if p.pos == before {
                            p.bump();
                        }
                    }
                    elements
                });
                self.expect(&TokenKind::RBracket, "to close a list");
                let span = Span::new(start, self.prev_end());
                let id = self.id();
                Expr {
                    id,
                    kind: ExprKind::List { elements },
                    span,
                }
            }
            TokenKind::LParen => {
                self.bump();
                let inner = self.nested(|p| p.expr_bp(0));
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
        let args = self.parse_args();
        // A policy is only ever written here, directly after a call, so it
        // needs no place in the precedence table.
        let policy = self.parse_policy();
        let span = Span::new(callee.span.lo, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
                policy,
            },
            span,
        }
    }

    /// `spawn f(args)`.
    ///
    /// The callee is parsed as a name, or as `namespace.name` so that spawning
    /// a capability reaches the type checker, which can say why it is not
    /// allowed. The grammar's job here is only to get the shape.
    fn parse_spawn(&mut self, start: u32) -> Expr {
        let name = self.expect_ident("a function name after `spawn`");
        let callee_span = name.span;
        let callee_id = self.id();
        let mut callee = Expr {
            id: callee_id,
            kind: ExprKind::Path(name),
            span: callee_span,
        };
        if self.at(&TokenKind::Dot) {
            callee = self.parse_field(callee);
        }
        let args = if self.eat(&TokenKind::LParen) {
            self.parse_args()
        } else {
            self.error(
                "E0205",
                "`spawn` needs a call",
                self.span(),
                "write `spawn f(...)`",
            );
            Vec::new()
        };
        let span = Span::new(start, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Spawn {
                callee: Box::new(callee),
                args,
            },
            span,
        }
    }

    /// The arguments of a call, with the opening parenthesis already consumed.
    fn parse_args(&mut self) -> Vec<Expr> {
        let args = self.nested(|p| {
            let mut args = Vec::new();
            while !p.at(&TokenKind::RParen) && !p.at_eof() {
                let before = p.pos;
                args.push(p.expr_bp(0));
                if !p.eat(&TokenKind::Comma) {
                    break;
                }
                if p.pos == before {
                    p.bump();
                }
            }
            args
        });
        self.expect(&TokenKind::RParen, "after the argument list");
        args
    }

    /// `Point { x: 1, y: 2 }`, with the name already consumed.
    fn parse_struct_literal(&mut self, name: Ident) -> Expr {
        let start = name.span.lo;
        self.bump(); // `{`
        let fields = self.nested(|p| {
            let mut fields = Vec::new();
            while !p.at(&TokenKind::RBrace) && !p.at_eof() {
                let before = p.pos;
                let field_start = p.span().lo;
                let field = p.expect_ident("a field name");
                p.expect(&TokenKind::Colon, "after a field name");
                let value = p.expr_bp(0);
                fields.push(FieldInit {
                    name: field,
                    value,
                    span: Span::new(field_start, p.prev_end()),
                });
                if !p.eat(&TokenKind::Comma) {
                    break;
                }
                if p.pos == before {
                    p.bump();
                }
            }
            fields
        });
        self.expect(&TokenKind::RBrace, "to close a struct literal");
        let span = Span::new(start, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Struct { name, fields },
            span,
        }
    }

    fn parse_index(&mut self, base: Expr) -> Expr {
        self.bump(); // `[`
        let index = self.nested(|p| p.expr_bp(0));
        self.expect(&TokenKind::RBracket, "to close an index");
        let span = Span::new(base.span.lo, self.prev_end());
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Index {
                base: Box::new(base),
                index: Box::new(index),
            },
            span,
        }
    }

    /// `retry N` and `timeout N`, in either order, each at most once.
    fn parse_policy(&mut self) -> CallPolicy {
        let mut policy = CallPolicy::default();
        let start = self.span().lo;
        while let TokenKind::Kw(kw @ (Keyword::Retry | Keyword::Timeout)) = self.peek() {
            let keyword = *kw;
            let keyword_span = self.bump().span;
            let value = self.parse_policy_number(keyword, keyword_span);
            let slot = match keyword {
                Keyword::Retry => &mut policy.attempts,
                _ => &mut policy.timeout_ms,
            };
            if slot.replace(value).is_some() {
                self.error(
                    "E0206",
                    format!("`{}` is given twice", keyword.text()),
                    keyword_span,
                    "already set on this call",
                );
            }
        }
        if !policy.is_empty() {
            policy.span = Some(Span::new(start, self.prev_end()));
        }
        policy
    }

    fn parse_policy_number(&mut self, keyword: Keyword, keyword_span: Span) -> u32 {
        match self.peek().clone() {
            TokenKind::Int(value) => {
                let span = self.bump().span;
                match u32::try_from(value) {
                    Ok(v) if v > 0 => v,
                    _ => {
                        self.error(
                            "E0207",
                            format!("`{}` needs a positive number", keyword.text()),
                            span,
                            "must fit in a 32-bit count",
                        );
                        1
                    }
                }
            }
            other => {
                let span = self.span();
                self.error(
                    "E0207",
                    format!("`{}` needs a number", keyword.text()),
                    span,
                    format!("found {}", other.describe()),
                );
                let _ = keyword_span;
                1
            }
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
