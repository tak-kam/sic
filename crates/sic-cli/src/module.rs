//! Turning an entry file and its imports into one module.
//!
//! Loading lives in the CLI because reading a file is an external effect, and
//! `sic-syntax` is not allowed one. What comes back is a single `Module`, so
//! every layer after this point still sees one program.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use sic_core::{Diagnostic, Label, SourceFile, SourceMap, Span};
use sic_syntax::ast::{Item, Module};

/// A program, gathered from every file it is built out of.
pub struct Loaded {
    /// Every file, the entry one first, in one offset space.
    pub sources: SourceMap,
    /// The items of every file, concatenated. `import` items are gone; the
    /// files they named are here instead.
    pub module: Module,
    pub diags: Vec<Diagnostic>,
}

/// Reads `path`, follows its imports, and parses all of them.
///
/// Only a failure to read the entry file is an `Err`: anything wrong with an
/// import is a diagnostic against the `import` that asked for it, because that
/// is where a reader can fix it.
pub fn load(
    path: &str,
    read: &dyn Fn(&str) -> Result<SourceFile, String>,
) -> Result<Loaded, String> {
    let mut ld = Loader {
        read,
        sources: SourceMap::new(),
        items: Vec::new(),
        diags: Vec::new(),
        loaded: HashMap::new(),
        stack: Vec::new(),
        ids: sic_syntax::NodeIds::new(),
    };
    let entry = ld.file_key(Path::new(path));
    let text = (ld.read)(path)?;
    ld.visit(path, text, entry);

    // Items arrive in the order they are loaded rather than in offset order,
    // because an import brings its file in where the `import` stands.
    let lo = ld.items.iter().map(|i| i.span().lo).min().unwrap_or(0);
    let hi = ld.items.iter().map(|i| i.span().hi).max().unwrap_or(0);
    let span = Span::new(lo, hi);
    Ok(Loaded {
        sources: ld.sources,
        module: Module {
            items: ld.items,
            span,
        },
        diags: ld.diags,
    })
}

struct Loader<'a> {
    read: &'a dyn Fn(&str) -> Result<SourceFile, String>,
    sources: SourceMap,
    items: Vec<Item>,
    diags: Vec<Diagnostic>,
    /// Every file already pulled in, so importing one twice brings it in once.
    loaded: HashMap<PathBuf, ()>,
    /// The chain of files currently being loaded, which is what makes a cycle
    /// visible.
    stack: Vec<PathBuf>,
    /// Node ids, carried across every file so that no two share one.
    ids: sic_syntax::NodeIds,
}

impl Loader<'_> {
    /// Parses one file and, depth first, everything it imports.
    fn visit(&mut self, path: &str, file: SourceFile, key: PathBuf) {
        self.loaded.insert(key.clone(), ());
        self.stack.push(key);

        let base = self.sources.add(file);
        let text = self.sources.files().last().unwrap().text().to_string();
        // One supply of ids for the whole program: two files that each numbered
        // from zero would collide in the checker's tables. See `NodeIds`.
        let (module, diags) = sic_syntax::parse_at(&text, base, &mut self.ids);
        self.diags.extend(diags);

        self.check_one_role(&module);

        let dir = Path::new(path)
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        for item in module.items {
            match item {
                Item::Import(decl) => self.follow(&decl.path, decl.span, &dir),
                other => self.items.push(other),
            }
        }
        self.stack.pop();
    }

    /// A file either grants capabilities or asks for them, never both.
    ///
    /// This is the rule that keeps the manifest readable: one file decides what
    /// the program may do, and the files it imports say what they need in order
    /// to do it. A library that could grant itself a capability would move that
    /// decision out of sight of whoever runs the program.
    fn check_one_role(&mut self, module: &Module) {
        let allow = module.items.iter().find(|i| matches!(i, Item::Allow(_)));
        let requires = module.items.iter().find(|i| matches!(i, Item::Requires(_)));
        if let (Some(allow), Some(requires)) = (allow, requires) {
            let mut d = Diagnostic::error(
                "E0403",
                "a file that both grants and requires capabilities",
                Label::new(requires.span(), "this file asks for capabilities"),
            );
            d.secondary
                .push(Label::new(allow.span(), "and grants them here"));
            d.notes.push(
                "a program's `allow` is the whole manifest; an imported file states what it needs with `requires`".to_string(),
            );
            self.diags.push(d);
        }
    }

    /// Resolves one `import` and loads what it names.
    fn follow(&mut self, raw: &str, span: Span, dir: &Path) {
        let Some(rel) = self.check_path(raw, span) else {
            return;
        };
        // `examples/./lib/x.sic` and `examples/lib/x.sic` are the same file,
        // and only one of them is worth putting in a message.
        let path = normalize(&dir.join(rel));
        let key = self.file_key(&path);
        let shown = display(&path);

        if self.stack.contains(&key) {
            let mut chain: Vec<String> = self.stack.iter().map(|p| display(p)).collect();
            chain.push(shown);
            self.diags.push(Diagnostic::error(
                "E0402",
                "an import cycle",
                Label::new(span, format!("{} imports itself", chain.join(" -> "))),
            ));
            return;
        }
        if self.loaded.contains_key(&key) {
            // Already here. Importing the same file from two places is normal
            // and brings it in once.
            return;
        }
        match (self.read)(&shown) {
            Ok(file) => self.visit(&shown, file, key),
            Err(msg) => self.diags.push(Diagnostic::error(
                "E0401",
                "an import that cannot be read",
                Label::new(span, msg),
            )),
        }
    }

    /// The rules an import path has to follow. They exist so that what a
    /// program is built from stays inside the directory it was started from.
    fn check_path(&mut self, raw: &str, span: Span) -> Option<PathBuf> {
        let mut reject = |why: &str| {
            self.diags.push(Diagnostic::error(
                "E0400",
                "an import path that cannot be used",
                Label::new(span, why.to_string()),
            ));
            None::<PathBuf>
        };
        if raw.is_empty() {
            return reject("the path is empty");
        }
        let path = Path::new(raw);
        if path.is_absolute() || raw.starts_with('/') || raw.starts_with('\\') {
            return reject("an import path is relative to the file that writes it");
        }
        for c in path.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return reject("`..` would reach outside the program's directory");
                }
                _ => return reject("only a relative path made of plain names is allowed"),
            }
        }
        if !raw.ends_with(".sic") {
            return reject("an imported file is named `*.sic`");
        }
        Some(path.to_path_buf())
    }

    /// What decides whether two import paths mean the same file. The real path
    /// when the filesystem can give one, and the path as written otherwise, so
    /// that a file that does not exist still gets one diagnostic per import.
    fn file_key(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
    }
}

/// `./a/./b.sic` and `a/b.sic` are the same file even before it exists.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A path as it should appear in a message. Paths that came from an import are
/// built from strings that were valid UTF-8, so the lossy case cannot be hit by
/// anything sic itself produced.
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
