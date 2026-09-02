//! Where a model's answer goes, read out of the bytecode.
//!
//! `E0372` refuses a program that passes a model's answer to a capability that
//! changes something. It refuses *source*. Trust types are erased before
//! bytecode - no section holds `LLM<T>` - so the sentence "this program cannot
//! do that" was true of every program this compiler compiled and was not a
//! property of the file. Three commands read bytecode and not source, and each
//! is a moment where somebody decides whether to trust a program.
//!
//! What a person approving a plan is worried about is not the manifest. It is
//! the *flow*: is there a path in this program from a model's answer to a file
//! being written. That is what was thrown away, and this is what recovers it -
//! from the instructions, without running any of them.
//!
//! It became answerable when `approve` stopped lowering to a `MOVE`. A `MOVE`
//! is what every assignment lowers to, so the laundering point was invisible
//! and could only have been recognised as a *shape* - a move guarded by a
//! branch on a `human.approve` whose `TO_JSON` named the same register - which
//! is a reader trusting the compiler's habits rather than reading a fact.
//! `Op::Approve` is the fact.
//!
//! # It over-reports rather than under-reports
//!
//! Deliberately, and in one specific way: the analysis is context-insensitive.
//! A function's parameter carries the taint of every call site joined together,
//! so a helper called once with a model's answer and once with a literal is
//! analysed as if both were the model's. A flow reported here may not be
//! reachable on any single path.
//!
//! That is the safe direction and the only one worth having. A plan that missed
//! a flow would be a false assurance about the one question this language
//! exists to answer, and a person reading "no model output reaches an effect"
//! has to be able to believe it.

use sic_bytecode::inst::Op;
use sic_bytecode::program::Program;
use sic_core::CapKind;

/// How much a value is to be worried about.
///
/// Ordered, and the order is the join: where two paths meet, the worse one
/// wins. `Approved` above `Clean` because a person agreeing to a value does not
/// make it a literal - a reader should still be told the path exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Taint {
    #[default]
    Clean,
    /// From a model, and a person said yes to it.
    Approved,
    /// From a model, and nobody was asked.
    FromModel,
}

/// A model's answer arriving at a capability that changes something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    /// The function the call is written in, and where.
    pub func: String,
    pub position: Option<(u32, u32)>,
    pub file: Option<String>,
    /// The capability being given the value.
    pub cap: String,
    pub kind: CapKind,
    /// Whether a person agreed to the value on every path that reaches here.
    pub approved: bool,
}

/// Every place a model's answer reaches a capability that changes something.
pub fn flows(program: &Program) -> Vec<Flow> {
    let state = solve(program);
    let mut found = Vec::new();
    for (index, func) in program.funcs.iter().enumerate() {
        for offset in 0..func.code_len as usize {
            let pc = func.code_off + offset as u32;
            let Some(inst) = program.code.get(pc as usize) else {
                continue;
            };
            let regs = state.at(index, offset);
            if inst.op() != Some(Op::CallCap) {
                continue;
            }
            let Some(cap) = program.caps.get(inst.b() as usize) else {
                continue;
            };
            if !changes_something(cap.kind) {
                continue;
            }
            let worst = (0..cap.params.len())
                .map(|i| taint_of(regs, inst.c().saturating_add(i as u8)))
                .max()
                .unwrap_or_default();
            if worst == Taint::Clean {
                continue;
            }
            found.push(Flow {
                func: func.name.clone(),
                position: program.debug.position(pc),
                file: program.debug.file(pc).map(str::to_string),
                cap: cap.name.clone(),
                kind: cap.kind,
                approved: worst == Taint::Approved,
            });
        }
    }
    found
}

/// Writing a file and running a program change something. Reading does not, and
/// `invoke` is asking - a model or a person - which is where the values in
/// question come from rather than somewhere they may not go.
fn changes_something(kind: CapKind) -> bool {
    matches!(kind, CapKind::Write | CapKind::Exec)
}

/// The taint of every register at every instruction, at a fixed point.
struct Solved {
    /// Per function, per instruction offset within it, the taint of each
    /// register on the way in.
    states: Vec<Vec<Vec<Taint>>>,
}

impl Solved {
    /// What the registers hold on the way into one instruction.
    fn at(&self, func: usize, offset: usize) -> &[Taint] {
        &self.states[func][offset]
    }
}

/// Flow-sensitive within a function, context-insensitive across them.
///
/// Flow-sensitive is not a refinement here; it is the whole thing working. The
/// compiler reuses one register window for every call's arguments, so a
/// register that once held a prompt later holds a path - and a summary that
/// joined a register over a whole body would report every program as carrying
/// a model's answer everywhere. It also could not tell a register before an
/// `APPROVE` from the same register after one, which is the distinction this
/// exists to draw.
///
/// Context-insensitive is a real approximation and is left in: a function's
/// parameters take the join of every call site, so a helper called once with a
/// model's answer and once with a literal is analysed as if both were the
/// model's. That over-reports, which is the direction to be wrong in.
fn solve(program: &Program) -> Solved {
    let counts: Vec<usize> = program.funcs.iter().map(|f| f.code_len as usize).collect();
    let mut states: Vec<Vec<Vec<Taint>>> = program
        .funcs
        .iter()
        .zip(&counts)
        .map(|(f, n)| vec![vec![Taint::Clean; f.reg_count as usize]; *n])
        .collect();
    let mut params: Vec<Vec<Taint>> = program
        .funcs
        .iter()
        .map(|f| vec![Taint::Clean; f.param_count()])
        .collect();
    let mut returns: Vec<Taint> = vec![Taint::Clean; program.funcs.len()];
    // The model is a name in the manifest rather than a kind: `human.approve`
    // is an `invoke` too, and what it answers with is a yes.
    let model: Vec<bool> = program
        .caps
        .iter()
        .map(|c| c.name == "llm.invoke")
        .collect();

    // Two nested fixed points. The inner one settles one function's registers
    // against the summaries it can see; the outer one settles the summaries.
    // Every step can only raise a taint and the lattice has three values, so
    // both terminate; the bounds are belt and braces against a lattice that
    // grows a value later.
    let outer = program.funcs.len().saturating_mul(3).saturating_add(8);
    for _ in 0..outer {
        let mut summaries_changed = false;
        for (index, func) in program.funcs.iter().enumerate() {
            let n = counts[index];
            if n == 0 {
                continue;
            }
            // A function starts with its parameters in the low registers, which
            // is the calling convention the VM uses.
            for (i, taint) in params[index].iter().enumerate() {
                if i < states[index][0].len() {
                    states[index][0][i] = states[index][0][i].max(*taint);
                }
            }
            let mut work: Vec<usize> = (0..n).collect();
            let inner = n.saturating_mul(6).saturating_add(16);
            let mut steps = 0usize;
            while let Some(offset) = work.pop() {
                steps += 1;
                if steps > inner.saturating_mul(n.max(1)) {
                    // Cannot happen while the lattice is three values and every
                    // step raises. Stopping beats looping on bytecode nobody
                    // has verified yet.
                    break;
                }
                let pc = func.code_off + offset as u32;
                let Some(inst) = program.code.get(pc as usize).copied() else {
                    continue;
                };
                let Some(op) = inst.op() else { continue };
                let (a, b, c) = (inst.a(), inst.b(), inst.c());
                let mut after = states[index][offset].clone();

                match op {
                    Op::CallCap => {
                        if model.get(b as usize).copied().unwrap_or(false) {
                            set(&mut after, a, Taint::FromModel);
                        } else {
                            // Anything else answers with its own value, and
                            // where that came from is not this question.
                            set(&mut after, a, Taint::Clean);
                        }
                    }
                    // The one instruction that lowers a taint, and the only
                    // thing in the file that says a person agreed.
                    Op::Approve => {
                        let out = match taint_of(&after, b) {
                            Taint::Clean => Taint::Clean,
                            _ => Taint::Approved,
                        };
                        set(&mut after, a, out);
                    }
                    Op::Call | Op::Spawn => {
                        if let Some(callee) = program.funcs.get(b as usize) {
                            let width = callee.param_count();
                            for (i, slot) in params[b as usize].iter_mut().enumerate().take(width) {
                                let arg = taint_of(&after, c.saturating_add(i as u8));
                                if arg > *slot {
                                    *slot = arg;
                                    summaries_changed = true;
                                }
                            }
                            set(&mut after, a, returns[b as usize]);
                        }
                    }
                    Op::Return => {
                        let out = taint_of(&after, a);
                        if out > returns[index] {
                            returns[index] = out;
                            summaries_changed = true;
                        }
                    }
                    _ => {
                        // The read windows are the verifier's. A `_` arm that
                        // guessed at one would be this analysis quietly losing
                        // a flow, which is the one way it must not be wrong.
                        let reads: Vec<u8> = match op {
                            Op::LoadConst | Op::Jump | Op::Halt => Vec::new(),
                            Op::JumpIf | Op::JumpIfNot | Op::Fail => vec![a],
                            Op::Log => vec![b],
                            Op::Move
                            | Op::Not
                            | Op::Await
                            | Op::Len
                            | Op::GetField
                            | Op::GetOpt
                            | Op::HasOpt => vec![b],
                            Op::FromJson | Op::ToJson => vec![c],
                            Op::MakeList => (0..c).map(|i| b.saturating_add(i)).collect(),
                            Op::MakeObject => program
                                .types
                                .get(b as usize)
                                .and_then(|t| t.fields())
                                .map(|f| (0..f.len()).map(|i| c.saturating_add(i as u8)).collect())
                                .unwrap_or_default(),
                            // Two operands: arithmetic, the comparisons,
                            // `CONCAT`, `GET_INDEX`, `CONTAINS`, `STARTS_WITH`.
                            _ => vec![b, c],
                        };
                        if !writes(op) {
                            // Nothing to assign; the state passes through.
                        } else {
                            let worst = reads
                                .iter()
                                .map(|r| taint_of(&after, *r))
                                .max()
                                .unwrap_or_default();
                            set(&mut after, a, worst);
                        }
                    }
                }

                for next in successors(func, pc, offset, op, inst, n) {
                    if next >= n {
                        continue;
                    }
                    let mut moved = false;
                    for (slot, incoming) in states[index][next].iter_mut().zip(&after) {
                        if *incoming > *slot {
                            *slot = *incoming;
                            moved = true;
                        }
                    }
                    if moved {
                        work.push(next);
                    }
                }
            }
        }
        if !summaries_changed {
            break;
        }
    }
    Solved { states }
}

/// Whether an instruction assigns to `a`.
fn writes(op: Op) -> bool {
    !matches!(
        op,
        Op::Jump | Op::JumpIf | Op::JumpIfNot | Op::Halt | Op::Log | Op::Fail | Op::Return
    )
}

/// Where control can go next, as offsets within the function.
fn successors(
    func: &sic_bytecode::program::FuncDef,
    pc: u32,
    offset: usize,
    op: Op,
    inst: sic_bytecode::Inst,
    count: usize,
) -> Vec<usize> {
    let target = || {
        let at = pc as i64 + 1 + inst.sbx() as i64 - func.code_off as i64;
        usize::try_from(at).unwrap_or(count)
    };
    match op {
        Op::Jump => vec![target()],
        Op::JumpIf | Op::JumpIfNot => vec![offset + 1, target()],
        // Nothing follows these, in this function.
        Op::Return | Op::Fail | Op::Halt => Vec::new(),
        _ => vec![offset + 1],
    }
}

fn set(regs: &mut [Taint], reg: u8, to: Taint) {
    if let Some(slot) = regs.get_mut(reg as usize) {
        *slot = to;
    }
}

fn taint_of(regs: &[Taint], reg: u8) -> Taint {
    regs.get(reg as usize).copied().unwrap_or_default()
}
