//! A recursive descent parser for items and statements. Expressions alone use a
//! Pratt parser (see the `expr` module).
//!
//! On error the parser does not stop: it skips to a synchronization point (`;`,
//! `}`, or a keyword that can start a statement or item) and keeps going.
//! Recovery stays deliberately shallow, because inventing a plausible AST is
//! worse than leaving a hole. Holes are `ExprKind::Error`.
//!
//! Recursive descent turns nesting in the source into depth on the stack, so
//! the one thing recovery cannot help with is a file that nests too deeply:
//! the process is gone before a diagnostic can be written. `MAX_DEPTH` is the
//! limit that keeps that from happening.

mod expr;

/// How deeply blocks, expressions and types may nest.
///
/// How long a path from the root of a tree this parser will build.
///
/// A `.sic` file is untrusted input in exactly the way a model's answer is, and
/// `sic-json` caps a document for the same reason. `sic plan` is the case that
/// decides it: its whole justification is that it is safe to run on a program
/// nobody has decided to trust yet, and safe has to include not dying on one.
///
/// This counts **tree** depth rather than parser recursion, and the difference
/// is the whole of the second half of this. Nesting - a parenthesis, a call
/// argument, an `if` inside an `if` - makes the parser recurse, and a counter on
/// that recursion is what caught it. A chain does not: `1 + 1 + 1 + ...` and
/// `a.f.f.f...` are read by a loop, so the parser stays shallow while the tree
/// gets one level deeper per operator. Every pass that walks the AST afterwards
/// recurses on that depth, and three thousand terms in a seven-kilobyte file
/// took the process down in `print::dump`, in name resolution and in type
/// checking.
///
/// One number rather than two, because the two are additive: a hundred nested
/// parentheses each holding a hundred-term sum is one path two hundred nodes
/// long. Bounding them separately would bound their product, which is not what
/// a stack cares about.
///
/// 256 is chosen from both sides. Above it, nothing a person writes: v0.1 has
/// no loops, so the deepest a body can nest is `if` inside `if`, and a function
/// with twenty of those would be unreadable long before the parser minded; a
/// sum of 256 terms or a chain of 256 field accesses is generated source, and a
/// generator that emits one has produced something no reviewer can read either.
/// Below it, the stack: the tightest shape measured on the musl build the
/// release ships overflows somewhere between 1855 (nested struct literals, in
/// the parser) and 2500 (a flat sum, in the passes that walk what the parser
/// built), so 256 leaves better than a factor of seven on the worst of them.
///
/// It refuses programs that were legal before, and that is the decision rather
/// than a side effect: an expression longer than this is refused so that every
/// consumer of the AST - including the ones not written yet - is protected by
/// one check instead of by four passes each remembering to use a work stack.
pub const MAX_DEPTH: u32 = 256;

use sic_core::{Answers, Diagnostic, Label, NodeId, Span};

use crate::ast::*;
use crate::lexer::tokenize_at;
use crate::token::{Keyword, Token, TokenKind};

/// A running supply of node ids.
///
/// Ids have to be unique across every file a program is built from, not only
/// within one. The checker keys `res_of` and `type_of` by `NodeId` and a
/// program is one module merged from all its files, so two files that each
/// numbered from zero give those tables two entries under one key - and the
/// second silently wins. What that produced was not a diagnostic: a capability
/// call lowered to a call of whatever the other file's node of the same id had
/// resolved to, and `sic plan` reported a program with no external effects, or
/// the lowering reached `unreachable!`.
///
/// So the supply is a value the caller carries from file to file, rather than
/// something each parse starts over.
#[derive(Debug, Default)]
pub struct NodeIds(u32);

impl NodeIds {
    pub fn new() -> NodeIds {
        NodeIds(0)
    }

    /// The next id that has not been handed out. For a test that wants to say
    /// where one file's ids stopped and the next file's began.
    pub fn peek(&self) -> u32 {
        self.0
    }
}

/// Parses a source text as a single module. The diagnostics include the lexer's.
pub fn parse(src: &str) -> (Module, Vec<Diagnostic>) {
    parse_at(src, 0, &mut NodeIds::new())
}

/// The same, for a file at `base` in a `SourceMap`'s offset space.
/// Parses one file of a program.
///
/// `base` puts this file's spans in the whole program's offset space; `ids`
/// does the same for its node ids, and is advanced by however many this file
/// needed.
pub fn parse_at(src: &str, base: u32, ids: &mut NodeIds) -> (Module, Vec<Diagnostic>) {
    let (tokens, mut diags) = tokenize_at(src, base);
    let mut p = Parser::new(tokens, ids.0);
    let module = p.parse_module();
    diags.append(&mut p.diags);
    ids.0 = p.next_id;
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
    /// How many levels of block, expression and type the parser is inside.
    depth: u32,
    /// Set when `MAX_DEPTH` was reached, which ends the parse.
    stopped: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>, first_id: u32) -> Self {
        Self {
            tokens,
            pos: 0,
            next_id: first_id,
            diags: Vec::new(),
            no_struct: 0,
            depth: 0,
            stopped: false,
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

    /// The token after the one `peek` returns. One is enough: the only place
    /// this parser needs to look past the next token is `log <level>`, where
    /// both words are ordinary identifiers.
    fn peek_next(&self) -> &TokenKind {
        match self.tokens.get(self.pos + 1) {
            Some(token) => &token.kind,
            None => &self.tokens[self.tokens.len() - 1].kind,
        }
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
        self.push(Diagnostic::error(code, msg, Label::new(span, label)));
    }

    /// Records a diagnostic, unless the depth limit has already ended the parse.
    fn push(&mut self, diag: Diagnostic) {
        if !self.stopped {
            self.diags.push(diag);
        }
    }

    /// Enters one level of nesting. `false` means the input nests deeper than
    /// `MAX_DEPTH`: it has been reported, and the parse is over.
    fn enter(&mut self) -> bool {
        self.depth += 1;
        if self.depth <= MAX_DEPTH {
            return true;
        }
        let span = self.span();
        self.error(
            "E0214",
            format!("nested more than {MAX_DEPTH} levels deep"),
            span,
            "the parser stops here",
        );
        // Nothing after this is worth reading. Every level on the way out is
        // about to notice its own delimiter is unclosed, and a hundred of those
        // hide the one line that says why; moving to the end of the input ends
        // each of their loops, and `stopped` keeps what they would have said
        // out of the output. One clear reason is the whole point.
        self.stopped = true;
        self.pos = self.tokens.len() - 1;
        false
    }

    /// Leaves the level `enter` reported entering.
    fn leave(&mut self) {
        self.depth -= 1;
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
        self.push(
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
    /// type Line { reason: String, .. }
    /// ```
    ///
    /// The `..` says the type describes part of a document rather than all of
    /// it. It comes last because that is where it reads as "and the rest":
    /// allowing it anywhere would make a reader look for it, and there is
    /// nothing it could mean in the middle that it does not mean at the end.
    fn parse_type_decl(&mut self) -> TypeDecl {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `type`
        let name = self.expect_ident("a type name");
        let mut fields = Vec::new();
        let mut open = false;
        if self.expect(&TokenKind::LBrace, "to open a type body") {
            while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                if self.at(&TokenKind::DotDot) {
                    let span = self.span();
                    self.bump();
                    open = true;
                    self.eat(&TokenKind::Comma);
                    if !self.at(&TokenKind::RBrace) && !self.at_eof() {
                        self.error(
                            "E0219",
                            "`..` has to be the last thing in a type body",
                            span,
                            "the fields the type declares come before it",
                        );
                        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
                            self.bump();
                        }
                    }
                    break;
                }
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
            open,
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
            tools: None,
            deadline_ms: None,
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

    /// One of the agent settings that is a positive count.
    ///
    /// They share a diagnostic because they share a mistake: a number that is
    /// missing, negative, or too large to be a count.
    fn parse_agent_count(
        &mut self,
        name: &str,
        set: impl Fn(&mut AgentDecl, u32),
        decl: &mut AgentDecl,
    ) {
        match self.peek().clone() {
            TokenKind::Int(value) => {
                let span = self.bump().span;
                match u32::try_from(value) {
                    Ok(v) if v > 0 => set(decl, v),
                    _ => self.error(
                        "E0208",
                        format!("`{name}` needs a positive number"),
                        span,
                        "must fit in a 32-bit count",
                    ),
                }
            }
            other => {
                let span = self.span();
                self.error(
                    "E0208",
                    format!("`{name}` needs a number"),
                    span,
                    format!("found {}", other.describe()),
                );
            }
        }
    }

    fn parse_agent_field(&mut self, decl: &mut AgentDecl) {
        let key = self.expect_ident("an agent setting");
        self.expect(&TokenKind::Colon, "after an agent setting");
        match key.name.as_str() {
            "input" => decl.input = Some(self.parse_type()),
            "output" => decl.output = Some(self.parse_type()),
            "budget" => {
                self.parse_agent_count(key.name.as_str(), |decl, v| decl.budget = Some(v), decl)
            }
            // The two bounds an agent with tools needs, and the two numbers
            // that were compiled into the driver before this: see
            // `docs/design/authority.md` §8.
            "tools" => {
                self.parse_agent_count(key.name.as_str(), |decl, v| decl.tools = Some(v), decl)
            }
            "deadline" => self.parse_agent_count(
                key.name.as_str(),
                |decl, v| decl.deadline_ms = Some(v),
                decl,
            ),
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
                        "E0215",
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
                    "expected `input`, `output`, `budget`, `tools`, `deadline` or `memory`",
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
        // `repeatable` says that performing this twice is the same as
        // performing it once. Without it, `retry` on a call to this capability
        // does not compile: what retrying an effect means is a claim about the
        // effect, and the manifest is where claims about effects live.
        // `delegable` says the agent answering this program's model calls may
        // use this capability too. Without it the grant is the program's
        // alone, which is the safe direction: what a program does with a
        // capability is written in the program, and what an agent would do
        // with one is written at run time by the agent.
        //
        // Both are claims the manifest makes and the language cannot check, so
        // both are words on the grant, and either may come first.
        //
        // `in "/abs/path"` and `env { NAME: "value" }` are the other two facts
        // a child process depends on. They are on the grant for the same
        // reason `args` is: a plan reads the manifest and runs nothing, so
        // what it can print is what the manifest says. See
        // `docs/design/capabilities.md`.
        //
        // `answers json` and `answers jsonl` say what form the program's
        // output takes, which is the one thing about a call the manifest could
        // not say and the plan could not print. The format is a bare
        // identifier and not a string - the one place this departs from
        // `sha256 "..."` - because a digest is unbounded data and a format is
        // one of two words, so `answers jsonl1` should be a diagnostic where it
        // is written rather than a string that means nothing to anybody until
        // the broker refuses it. See `docs/design/answers.md` §11.
        //
        // All five may come in any order, and each at most once.
        let mut repeatable = false;
        let mut delegable = false;
        let mut dir = None;
        let mut env = Vec::new();
        let mut saw_env = false;
        let mut answers = None;
        loop {
            match self.peek().clone() {
                TokenKind::Ident(name) if name == "repeatable" && !repeatable => {
                    self.bump();
                    repeatable = true;
                }
                TokenKind::Ident(name) if name == "delegable" && !delegable => {
                    self.bump();
                    delegable = true;
                }
                TokenKind::Kw(Keyword::In) if dir.is_none() => {
                    self.bump();
                    match self.peek().clone() {
                        TokenKind::Str(text) => {
                            let span = self.bump().span;
                            dir = Some(Ident2 { text, span });
                        }
                        other => {
                            let span = self.span();
                            self.error(
                                "E0216",
                                "`in` needs a directory",
                                span,
                                format!("found {}", other.describe()),
                            );
                            break;
                        }
                    }
                }
                TokenKind::Ident(name) if name == "env" && !saw_env => {
                    self.bump();
                    saw_env = true;
                    env = self.parse_grant_env();
                }
                TokenKind::Ident(name) if name == "answers" && answers.is_none() => {
                    self.bump();
                    answers = self.parse_grant_answers();
                    if answers.is_none() {
                        break;
                    }
                }
                _ => break,
            }
        }
        if !self.expect(&TokenKind::Semi, "after a capability grant") {
            self.recover_to_grant_end();
        }
        CapGrant {
            id,
            path,
            constraint,
            sha256,
            args,
            repeatable,
            delegable,
            dir,
            env,
            answers,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// `json` or `jsonl`, the two forms a grant can claim its program answers
    /// in.
    ///
    /// There is no third, and a word that is neither is refused here rather
    /// than carried to a broker that would refuse it on the first run. The two
    /// are the rungs `docs/design/answers.md` §2 settled on; the typed rung
    /// above them is refused by §3 with four reasons.
    fn parse_grant_answers(&mut self) -> Option<AnswersClause> {
        match self.peek().clone() {
            TokenKind::Ident(word) => {
                let span = self.bump().span;
                match Answers::from_word(&word) {
                    Some(shape) => Some(AnswersClause { shape, span }),
                    None => {
                        self.push(
                            Diagnostic::error(
                                "E0220",
                                "`answers` takes `json` or `jsonl`",
                                Label::new(
                                    span,
                                    format!("`{word}` is not a format the broker can check"),
                                ),
                            )
                            .with_note(
                                "`json` is the whole output as one document, `jsonl` is one \
                                 value per line",
                            ),
                        );
                        None
                    }
                }
            }
            other => {
                let span = self.span();
                self.error(
                    "E0220",
                    "`answers` takes `json` or `jsonl`",
                    span,
                    format!("found {}", other.describe()),
                );
                None
            }
        }
    }

    /// `{ RUSTFLAGS: "-C debuginfo=0" }`: the environment the child is given.
    ///
    /// Names are identifiers and values are string literals, for the same
    /// reason `args` takes only literals: a grant is read before anything runs,
    /// so anything a plan cannot print does not belong in one.
    fn parse_grant_env(&mut self) -> Vec<(Ident2, Ident2)> {
        let mut out = Vec::new();
        if !self.expect(&TokenKind::LBrace, "after `env`") {
            return out;
        }
        loop {
            match self.peek().clone() {
                TokenKind::RBrace => {
                    self.bump();
                    return out;
                }
                TokenKind::Ident(name) => {
                    let span = self.bump().span;
                    let key = Ident2 { text: name, span };
                    if !self.expect(&TokenKind::Colon, "after an environment variable's name") {
                        return out;
                    }
                    match self.peek().clone() {
                        TokenKind::Str(text) => {
                            let span = self.bump().span;
                            out.push((key, Ident2 { text, span }));
                        }
                        other => {
                            let span = self.span();
                            self.error(
                                "E0217",
                                "an environment variable's value is a string",
                                span,
                                format!("found {}", other.describe()),
                            );
                            return out;
                        }
                    }
                    if self.at(&TokenKind::Comma) {
                        self.bump();
                    }
                }
                other => {
                    let span = self.span();
                    self.error(
                        "E0217",
                        "`env` takes `NAME: \"value\"` pairs",
                        span,
                        format!("found {}", other.describe()),
                    );
                    return out;
                }
            }
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
        // `List<List<List<...>>>` recurses once per argument list.
        if !self.enter() {
            return error_type(id, Span::empty(start));
        }
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
        self.leave();
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
        // A block reaches another block through the `if` inside it.
        if !self.enter() {
            return error_block(id, Span::empty(start));
        }
        if !self.expect(&TokenKind::LBrace, "to open a block") {
            self.leave();
            return error_block(id, Span::empty(start));
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
        self.leave();
        Block {
            id,
            stmts,
            span: Span::new(start, self.prev_end()),
        }
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        // `log info ...` is two identifiers in a row, which no expression can
        // be, so `log` stays an ordinary identifier and a function may still be
        // called that. Same reason `args` and `sha256` are not keywords.
        if let (TokenKind::Ident(name), TokenKind::Ident(_)) = (self.peek(), self.peek_next()) {
            // Two identifiers in a row, which no expression can be - so this
            // is a `log` statement whatever the second word is, and a word
            // that is not a level is a mistyped level rather than a sentence
            // the parser has to guess at.
            if name == "log" {
                return Some(self.parse_log());
            }
        }
        match self.peek() {
            TokenKind::Kw(Keyword::Let) => Some(self.parse_let()),
            TokenKind::Kw(Keyword::Return) => Some(self.parse_return()),
            TokenKind::Kw(Keyword::If) => Some(Stmt::If(self.parse_if())),
            TokenKind::Kw(Keyword::For) => Some(Stmt::For(self.parse_for())),
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

    /// `log <level> <expr>;`
    ///
    /// The level is one of four words and the message is an expression, so a
    /// program can say what happened rather than only that something did.
    fn parse_log(&mut self) -> Stmt {
        let id = self.id();
        let start = self.span().lo;
        self.bump(); // `log`
        let TokenKind::Ident(name) = self.peek().clone() else {
            unreachable!("parse_stmt only calls this when a word follows `log`");
        };
        let span = self.span();
        self.bump();
        let level = match LogLevel::from_name(&name) {
            Some(level) => level,
            None => {
                self.error(
                    "E0218",
                    format!("`{name}` is not a log level"),
                    span,
                    "the levels are `debug`, `info`, `warn` and `error`",
                );
                LogLevel::Info
            }
        };
        let message = self.parse_expr();
        if !self.expect(&TokenKind::Semi, "after a `log` statement") {
            self.recover_to_stmt_end();
        }
        Stmt::Log {
            id,
            level,
            message,
            span: Span::new(start, self.prev_end()),
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
        // An `else if` chain recurses here rather than through a block, so the
        // guard on `parse_block` alone would not see it.
        if !self.enter() {
            let span = Span::empty(start);
            return IfStmt {
                id,
                cond: self.error_expr(span),
                then_block: error_block(self.id(), span),
                else_branch: None,
                span,
            };
        }
        self.bump(); // `if`
        // `if Point { .. }` would be ambiguous with the body that follows, so a
        // struct literal is not allowed here. Parentheses make it legal again.
        let cond = self.parse_before_block();
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
        self.leave();
        IfStmt {
            id,
            cond,
            then_block,
            else_branch,
            span: Span::new(start, self.prev_end()),
        }
    }

    /// `for IDENT in expr block`.
    ///
    /// The binding is a plain identifier: there is no pattern to destructure
    /// with and no `mut` to ask for, so anything else in that position is a
    /// mistake rather than a shape the grammar has to allow.
    fn parse_for(&mut self) -> ForStmt {
        let id = self.id();
        let start = self.span().lo;
        // A loop reaches another loop, and its body is read by `parse_block`,
        // which counts its own level - but the header expression is read
        // before that, so this is where the nesting has to be counted.
        if !self.enter() {
            let span = Span::empty(start);
            return ForStmt {
                id,
                var: Ident {
                    name: String::new(),
                    span,
                },
                iter: self.error_expr(span),
                body: error_block(self.id(), span),
                span,
            };
        }
        self.bump(); // `for`
        let var = self.expect_ident("a loop variable");
        self.expect(&TokenKind::Kw(Keyword::In), "in a `for` loop");
        // `for x in Point { .. }` would be ambiguous with the body that
        // follows, the same way an `if` condition is. Parentheses make a
        // struct literal legal again.
        let iter = self.parse_before_block();
        let body = self.parse_block();
        self.leave();
        ForStmt {
            id,
            var,
            iter,
            body,
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
                | TokenKind::Kw(
                    Keyword::Fn | Keyword::Let | Keyword::Return | Keyword::If | Keyword::For,
                ) => {
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

/// The hole left where a block could not be read. An empty body is what a
/// block with no `{` already recovers to.
fn error_block(id: NodeId, span: Span) -> Block {
    Block {
        id,
        stmts: Vec::new(),
        span,
    }
}

/// The hole left where a type could not be read. An empty name is what a
/// missing type name already recovers to.
fn error_type(id: NodeId, span: Span) -> TypeExpr {
    TypeExpr {
        id,
        name: Ident {
            name: String::new(),
            span,
        },
        args: Vec::new(),
        span,
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
