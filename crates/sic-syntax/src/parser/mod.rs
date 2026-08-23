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
use crate::lexer::tokenize_at;
use crate::token::{Keyword, Token, TokenKind};

/// Parses a source text as a single module. The diagnostics include the lexer's.
pub fn parse(src: &str) -> (Module, Vec<Diagnostic>) {
    parse_at(src, 0)
}

/// The same, for a file at `base` in a `SourceMap`'s offset space.
pub fn parse_at(src: &str, base: u32) -> (Module, Vec<Diagnostic>) {
    let (tokens, mut diags) = tokenize_at(src, base);
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
    /// Non-zero while parsing somewhere a `{` would be read as the start of a
    /// block rather than of a struct literal.
    no_struct: u32,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            next_id: 0,
            diags: Vec::new(),
            no_struct: 0,
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
                TokenKind::Kw(Keyword::Allow) => items.push(Item::Allow(self.parse_allow())),
                TokenKind::Kw(Keyword::Type) => items.push(Item::Type(self.parse_type_decl())),
                TokenKind::Kw(Keyword::Agent) => items.push(Item::Agent(self.parse_agent())),
                TokenKind::Kw(Keyword::Import) => items.push(Item::Import(self.parse_import())),
                TokenKind::Kw(Keyword::Requires) => {
                    items.push(Item::Requires(self.parse_requires()))
                }
                other => {
                    let span = self.span();
                    let found = other.describe();
                    self.error(
                        "E0202",
                        "the top level holds `import`, `fn`, `type`, `agent`, `allow` and `requires`",
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

    /// Skips ahead to the next item.
    fn recover_to_item(&mut self) {
        while !self.at_eof() && !matches!(self.peek(), TokenKind::Kw(Keyword::Fn | Keyword::Allow))
        {
            self.bump();
        }
    }

    /// ```text
    /// type Point { x: Int, y: Int }
    /// ```
    fn parse_type_decl(&mut self) -> TypeDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `type`
        let name = self.expect_ident("a type name");
        let mut fields = Vec::new();
        if self.expect(&TokenKind::LBrace, "to open a type body") {
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                let before = self.pos;
                fields.push(self.parse_field_decl());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RBrace, "to close the type body");
        }
        TypeDecl {
            id,
            name,
            fields,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// `import "./lib/deploy.sic";`
    fn parse_import(&mut self) -> ImportDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `import`
        let path = match self.peek().clone() {
            TokenKind::Str(text) => {
                self.bump();
                text
            }
            other => {
                let span = self.span();
                self.error(
                    "E0212",
                    "`import` needs a path",
                    span,
                    format!("found {}", other.describe()),
                );
                String::new()
            }
        };
        self.expect(&TokenKind::Semi, "after an import");
        ImportDecl {
            id,
            path,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// ```text
    /// requires { process.exec; }
    /// ```
    fn parse_requires(&mut self) -> RequiresDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `requires`
        let mut caps = Vec::new();
        if self.expect(&TokenKind::LBrace, "to open a `requires` block") {
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                let before = self.pos;
                let cap_start = self.span().lo;
                let namespace = self.expect_ident("a capability namespace");
                self.expect(
                    &TokenKind::Dot,
                    "between a capability namespace and its name",
                );
                let name = self.expect_ident("a capability name");
                caps.push(CapPath {
                    namespace,
                    name,
                    span: Span::new(cap_start, self.prev_end()),
                });
                if !self.expect(&TokenKind::Semi, "after a required capability") {
                    self.recover_to_grant_end();
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RBrace, "to close the `requires` block");
        }
        RequiresDecl {
            id,
            caps,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// ```text
    /// agent diagnose { input: String, output: Diagnosis, budget: 8, memory: task }
    /// ```
    fn parse_agent(&mut self) -> AgentDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `agent`
        let name = self.expect_ident("an agent name");
        let mut decl = AgentDecl {
            id,
            name,
            input: None,
            output: None,
            budget: None,
            memory: false,
            span: Span::empty(start),
        };
        if self.expect(&TokenKind::LBrace, "to open an agent body") {
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                let before = self.pos;
                self.parse_agent_field(&mut decl);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RBrace, "to close the agent body");
        }
        decl.span = Span::new(start, self.prev_end());
        decl
    }

    fn parse_agent_field(&mut self, decl: &mut AgentDecl) {
        let key = self.expect_ident("an agent setting");
        self.expect(&TokenKind::Colon, "after an agent setting");
        match key.name.as_str() {
            "input" => decl.input = Some(self.parse_type()),
            "output" => decl.output = Some(self.parse_type()),
            "budget" => match self.peek().clone() {
                TokenKind::Int(value) => {
                    let span = self.bump().span;
                    match u32::try_from(value) {
                        Ok(v) if v > 0 => decl.budget = Some(v),
                        _ => self.error(
                            "E0208",
                            "`budget` needs a positive number of calls",
                            span,
                            "must fit in a 32-bit count",
                        ),
                    }
                }
                other => {
                    let span = self.span();
                    self.error(
                        "E0208",
                        "`budget` needs a number",
                        span,
                        format!("found {}", other.describe()),
                    );
                }
            },
            // `task` is the only scope there is. A conversation that lasted a
            // whole run would be one a program that never spawns already has,
            // and one that lasted a call is what not writing this means.
            "memory" => match self.peek().clone() {
                TokenKind::Ident(word) if word == "task" => {
                    self.bump();
                    decl.memory = true;
                }
                other => {
                    let span = self.span();
                    self.error(
                        "E0210",
                        "`memory` takes `task`",
                        span,
                        format!("found {}, and `task` is the only scope", other.describe()),
                    );
                }
            },
            other => {
                self.error(
                    "E0209",
                    format!("`{other}` is not an agent setting"),
                    key.span,
                    "expected `input`, `output`, `budget` or `memory`",
                );
                // Skip whatever it was, so one unknown setting does not
                // derail the rest of the body.
                while !self.at_eof() && !matches!(self.peek(), TokenKind::Comma | TokenKind::RBrace)
                {
                    self.bump();
                }
            }
        }
    }

    fn parse_field_decl(&mut self) -> FieldDecl {
        let id = self.id();
        let start = self.span().lo;
        let name = self.expect_ident("a field name");
        self.expect(&TokenKind::Colon, "before a field type");
        let ty = self.parse_type();
        FieldDecl {
            id,
            name,
            ty,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// ```text
    /// allow { fs.read "./input.txt"; process.exec "/usr/bin/true"; }
    /// ```
    fn parse_allow(&mut self) -> AllowDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `allow`
        let mut grants = Vec::new();
        if self.expect(&TokenKind::LBrace, "to open an `allow` block") {
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                let before = self.pos;
                grants.push(self.parse_grant());
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RBrace, "to close the `allow` block");
        }
        AllowDecl {
            id,
            grants,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_grant(&mut self) -> CapGrant {
        let id = self.id();
        let start = self.span().lo;
        let namespace = self.expect_ident("a capability namespace");
        self.expect(
            &TokenKind::Dot,
            "between a capability namespace and its name",
        );
        let name = self.expect_ident("a capability name");
        let path = CapPath {
            namespace,
            name,
            span: Span::new(start, self.prev_end()),
        };

        // The constraint is optional in the grammar; whether a capability can
        // be granted without one is for the checker to decide.
        let constraint = match self.peek().clone() {
            TokenKind::Str(text) => {
                self.bump();
                Some(text)
            }
            _ => None,
        };
        // `args ["send-keys", "-t", "sic:0"]` pins what the argument vector
        // has to start with. Like `sha256`, it is an ordinary identifier.
        let args = match self.peek().clone() {
            TokenKind::Ident(name) if name == "args" => {
                self.bump();
                self.parse_grant_args()
            }
            _ => Vec::new(),
        };
        // `sha256 "..."` pins what may run. It is an ordinary identifier, so
        // nothing is reserved for it.
        let sha256 = match self.peek().clone() {
            TokenKind::Ident(name) if name == "sha256" => {
                self.bump();
                match self.peek().clone() {
                    TokenKind::Str(text) => {
                        let span = self.bump().span;
                        Some(Ident2 { text, span })
                    }
                    other => {
                        let span = self.span();
                        self.error(
                            "E0211",
                            "`sha256` needs a digest",
                            span,
                            format!("found {}", other.describe()),
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        if !self.expect(&TokenKind::Semi, "after a capability grant") {
            self.recover_to_grant_end();
        }
        CapGrant {
            id,
            path,
            constraint,
            sha256,
            args,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// `["send-keys", "-t", "sic:0"]`: the strings a call's arguments have to
    /// start with. Only literals, because a grant is read before anything runs.
    fn parse_grant_args(&mut self) -> Vec<Ident2> {
        let mut out = Vec::new();
        if !self.expect(&TokenKind::LBracket, "after `args`") {
            return out;
        }
        loop {
            match self.peek().clone() {
                TokenKind::RBracket => {
                    self.bump();
                    return out;
                }
                TokenKind::Str(text) => {
                    let span = self.bump().span;
                    out.push(Ident2 { text, span });
                    if self.at(&TokenKind::Comma) {
                        self.bump();
                    }
                }
                other => {
                    let span = self.span();
                    self.error(
                        "E0213",
                        "`args` takes a list of strings",
                        span,
                        format!("found {}", other.describe()),
                    );
                    return out;
                }
            }
        }
    }

    /// Inside an `allow` block, a grant starts with an identifier and nothing
    /// else does, so an identifier is a synchronization point as well as `;`
    /// and the closing brace.
    fn recover_to_grant_end(&mut self) {
        while !self.at_eof() {
            match self.peek() {
                TokenKind::Semi => {
                    self.bump();
                    return;
                }
                TokenKind::Ident(_)
                | TokenKind::RBrace
                | TokenKind::Kw(Keyword::Fn | Keyword::Allow) => return,
                _ => {
                    self.bump();
                }
            }
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
        // `if Point { .. }` would be ambiguous with the body that follows, so a
        // struct literal is not allowed here. Parentheses make it legal again.
        let cond = self.parse_condition();
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
