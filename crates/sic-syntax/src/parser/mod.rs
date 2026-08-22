//! A recursive descent parser for items and statements. Expressions alone use a
//! Pratt parser (see the `expr` module).
//!
//! On error the parser does not stop: it skips to a synchronization point (`;`,
//! `}`, or a keyword that can start a statement or item) and keeps going.
//! Recovery stays deliberately shallow, because inventing a plausible AST is
//! worse than leaving a hole. Holes are `ExprKind::Error`.

mod expr;

use sic_core::{Diagnostic, Label, NodeId, Span};

use crate::ast::*;
use crate::lexer::tokenize;
use crate::token::{Keyword, Token, TokenKind};

/// Parses a source text as a single module. The diagnostics include the lexer's.
pub fn parse(src: &str) -> (Module, Vec<Diagnostic>) {
    let (tokens, mut diags) = tokenize(src);
    let mut p = Parser::new(tokens);
    let module = p.parse_module();
    diags.append(&mut p.diags);
    (module, diags)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    next_id: u32,
    diags: Vec<Diagnostic>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            next_id: 0,
            diags: Vec::new(),
        }
    }

    // ---- primitives ----

    fn id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    /// The end of the token that was consumed last, used to close spans.
    fn prev_end(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.hi
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, msg, Label::new(span, label)));
    }

    /// Requires a token. If it is not there, reports it and consumes nothing.
    fn expect(&mut self, kind: &TokenKind, ctx: &str) -> bool {
        if self.eat(kind) {
            return true;
        }
        // A missing token is reported just after the previous one. Pointing at
        // the current token would report a missing `;` at the start of the next
        // line, which hides the line that actually needs fixing.
        let found = self.peek().describe();
        let span = if self.pos == 0 {
            self.span()
        } else {
            Span::empty(self.prev_end())
        };
        let text = kind_text(kind);
        self.diags.push(
            Diagnostic::error(
                "E0200",
                format!("expected `{text}` {ctx}"),
                Label::new(span, format!("insert `{text}` here")),
            )
            .with_note(format!("found {found}")),
        );
        false
    }

    /// Requires an identifier, with a dedicated diagnostic for reserved words.
    fn expect_ident(&mut self, ctx: &str) -> Ident {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                Ident { name, span }
            }
            TokenKind::Kw(kw) if kw.is_reserved() => {
                let span = self.bump().span;
                self.error(
                    "E0210",
                    format!("`{}` is reserved for future use", kw.text()),
                    span,
                    "cannot be used as an identifier",
                );
                Ident {
                    name: kw.text().to_string(),
                    span,
                }
            }
            other => {
                let span = self.span();
                self.error(
                    "E0201",
                    format!("expected {ctx}"),
                    span,
                    format!("found {}", other.describe()),
                );
                Ident {
                    name: String::new(),
                    span: Span::empty(span.lo),
                }
            }
        }
    }

    // ---- items ----

    fn parse_module(&mut self) -> Module {
        let start = self.span().lo;
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            match self.peek() {
                TokenKind::Kw(Keyword::Fn) => items.push(Item::Fn(self.parse_fn())),
                other => {
                    let span = self.span();
                    let found = other.describe();
                    self.error(
                        "E0202",
                        "only function definitions are allowed at the top level",
                        span,
                        format!("found {found}"),
                    );
                    self.recover_to_item();
                }
            }
            // Guarantee progress even when recovery made none, so a malformed
            // input cannot loop forever.
            if self.pos == before {
                self.bump();
            }
        }
        Module {
            items,
            span: Span::new(start, self.prev_end().max(start)),
        }
    }

    /// Skips ahead to the next `fn`.
    fn recover_to_item(&mut self) {
        while !self.at_eof() && !matches!(self.peek(), TokenKind::Kw(Keyword::Fn)) {
            self.bump();
        }
    }

    fn parse_fn(&mut self) -> FnDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `fn`
        let name = self.expect_ident("a function name");

        let mut params = Vec::new();
        if self.expect(&TokenKind::LParen, "before the parameter list") {
            while !self.at(&TokenKind::RParen) && !self.at_eof() {
                let before = self.pos;
                params.push(self.parse_param());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RParen, "after the parameter list");
        }

        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };

        let body = self.parse_block();
        FnDecl {
            id,
            name,
            params,
            ret,
            body,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_param(&mut self) -> Param {
        let id = self.id();
        let start = self.span().lo;
        let name = self.expect_ident("a parameter name");
        self.expect(&TokenKind::Colon, "before a parameter type");
        let ty = self.parse_type();
        Param {
            id,
            name,
            ty,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_type(&mut self) -> TypeExpr {
        let id = self.id();
        let start = self.span().lo;
        let name = self.expect_ident("a type name");
        let mut args = Vec::new();
        if self.eat(&TokenKind::Lt) {
            while !self.at(&TokenKind::Gt) && !self.at_eof() {
                let before = self.pos;
                args.push(self.parse_type());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::Gt, "after the type arguments");
        }
        TypeExpr {
            id,
            name,
            args,
            span: Span::new(start, self.prev_end()),
        }
    }

    // ---- statements ----

    fn parse_block(&mut self) -> Block {
        let id = self.id();
        let start = self.span().lo;
        if !self.expect(&TokenKind::LBrace, "to open a block") {
            return Block {
                id,
                stmts: Vec::new(),
                span: Span::empty(start),
            };
        }
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            if let Some(s) = self.parse_stmt() {
                stmts.push(s);
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace, "to close the block");
        Block {
            id,
            stmts,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        match self.peek() {
            TokenKind::Kw(Keyword::Let) => Some(self.parse_let()),
            TokenKind::Kw(Keyword::Return) => Some(self.parse_return()),
            TokenKind::Kw(Keyword::If) => Some(Stmt::If(self.parse_if())),
            TokenKind::LBrace => {
                // A bare block is rejected in v0.1: what it is meant to scope is
                // ambiguous while there is nothing to scope.
                let span = self.span();
                self.error(
                    "E0203",
                    "a block statement is not allowed here",
                    span,
                    "in v0.1 a block can only be the body of `fn` or `if`",
                );
                self.recover_to_stmt_end();
                None
            }
            _ => {
                let id = self.id();
                let start = self.span().lo;
                let expr = self.parse_expr();
                if !self.expect(&TokenKind::Semi, "after an expression statement") {
                    self.recover_to_stmt_end();
                }
                Some(Stmt::Expr {
                    id,
                    expr,
                    span: Span::new(start, self.prev_end()),
                })
            }
        }
    }

    fn parse_let(&mut self) -> Stmt {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `let`
        let name = self.expect_ident("a variable name");
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type())
        } else {
            None
        };
        let init = if self.expect(&TokenKind::Eq, "in a `let` binding") {
            self.parse_expr()
        } else {
            // v0.1 has no uninitialized bindings, so recover with a hole.
            let span = Span::empty(self.span().lo);
            self.error_expr(span)
        };
        if !self.expect(&TokenKind::Semi, "after a `let` statement") {
            self.recover_to_stmt_end();
        }
        Stmt::Let {
            id,
            name,
            ty,
            init,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_return(&mut self) -> Stmt {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `return`
        let value = if self.at(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr())
        };
        if !self.expect(&TokenKind::Semi, "after a `return` statement") {
            self.recover_to_stmt_end();
        }
        Stmt::Return {
            id,
            value,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_if(&mut self) -> IfStmt {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `if`
        let cond = self.parse_expr();
        let then_block = self.parse_block();
        let else_branch = if self.eat(&TokenKind::Kw(Keyword::Else)) {
            if matches!(self.peek(), TokenKind::Kw(Keyword::If)) {
                Some(Box::new(ElseBranch::If(self.parse_if())))
            } else {
                Some(Box::new(ElseBranch::Block(self.parse_block())))
            }
        } else {
            None
        };
        IfStmt {
            id,
            cond,
            then_block,
            else_branch,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// Advances to a synchronization point. A `;` is consumed; every other
    /// synchronization point is left in place.
    ///
    /// Including the statement keywords matters: without them a single missing
    /// `;` would swallow the statement that follows it.
    fn recover_to_stmt_end(&mut self) {
        while !self.at_eof() {
            match self.peek() {
                TokenKind::Semi => {
                    self.bump();
                    return;
                }
                TokenKind::RBrace
                | TokenKind::Kw(Keyword::Fn | Keyword::Let | Keyword::Return | Keyword::If) => {
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn error_expr(&mut self, span: Span) -> Expr {
        let id = self.id();
        Expr {
            id,
            kind: ExprKind::Error,
            span,
        }
    }
}

fn kind_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(_) => "identifier".into(),
        other => other.describe().trim_matches('`').to_string(),
    }
}

#[cfg(test)]
mod tests;
