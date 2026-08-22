//! Runtime values and the arena that owns the heap ones.
//!
//! A `Value` is a small copyable descriptor; anything larger lives in the arena
//! and is referred to by a `Handle`. Keeping the values flat is what will let a
//! suspended run be written out and read back: there is nothing in here that
//! points outside the VM.

/// A reference to something in the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(Handle),
    /// A running computation. The number is a task in this run, and means
    /// nothing outside it.
    Task(u32),
    List(Handle),
    Object(Handle),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "Unit",
            Value::Bool(_) => "Bool",
            Value::I64(_) => "Int",
            Value::F64(_) => "Float",
            Value::Str(_) => "String",
            Value::Task(_) => "Task",
            Value::List(_) => "List",
            Value::Object(_) => "Object",
        }
    }

    pub fn display(&self, arena: &Arena) -> String {
        match self {
            Value::Unit => "unit".into(),
            Value::Bool(v) => format!("{v}"),
            Value::I64(v) => format!("{v}"),
            Value::F64(v) => format!("{v}"),
            Value::Str(h) => format!("{:?}", arena.str(*h)),
            Value::Task(id) => format!("<task {id}>"),
            Value::List(h) => {
                let items: Vec<String> = arena.list(*h).iter().map(|v| v.display(arena)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Object(h) => {
                let fields: Vec<String> =
                    arena.object(*h).iter().map(|v| v.display(arena)).collect();
                format!("{{{}}}", fields.join(", "))
            }
        }
    }
}

/// Heap storage for one run.
///
/// There is no garbage collector: a workflow allocates, runs, and the whole
/// arena is dropped at the end. Reclaiming memory mid-run is a problem for the
/// phase that has programs long enough to need it.
#[derive(Debug, Default, Clone)]
pub struct Arena {
    strings: Vec<String>,
    lists: Vec<Vec<Value>>,
    objects: Vec<Vec<Value>>,
}

impl Arena {
    pub fn alloc_str(&mut self, s: String) -> Handle {
        self.strings.push(s);
        Handle(self.strings.len() as u32 - 1)
    }

    /// The string behind a handle, or `""` when the handle is not one this
    /// arena issued.
    pub fn str(&self, h: Handle) -> &str {
        self.strings
            .get(h.0 as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Every string in the arena, for writing a checkpoint.
    pub fn alloc_list(&mut self, values: Vec<Value>) -> Handle {
        self.lists.push(values);
        Handle(self.lists.len() as u32 - 1)
    }

    /// The elements behind a list handle, or nothing for a handle this arena
    /// did not issue.
    pub fn list(&self, h: Handle) -> &[Value] {
        self.lists
            .get(h.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn alloc_object(&mut self, fields: Vec<Value>) -> Handle {
        self.objects.push(fields);
        Handle(self.objects.len() as u32 - 1)
    }

    pub fn object(&self, h: Handle) -> &[Value] {
        self.objects
            .get(h.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn lists(&self) -> &[Vec<Value>] {
        &self.lists
    }

    pub fn objects(&self) -> &[Vec<Value>] {
        &self.objects
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    /// Rebuilds an arena from a checkpoint. Handles keep their meaning because
    /// the order is preserved.
    pub fn from_parts(
        strings: Vec<String>,
        lists: Vec<Vec<Value>>,
        objects: Vec<Vec<Value>>,
    ) -> Self {
        Self {
            strings,
            lists,
            objects,
        }
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}
