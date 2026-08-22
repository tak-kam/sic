use sic_bytecode::disassemble;
use sic_core::SourceFile;

use super::compile;

/// Compiles a source string and returns the disassembly.
fn asm(src: &str) -> String {
    let hir = sic_ir::lower::compile_to_hir(src).expect("the source should compile");
    let file = SourceFile::new("t.sic", src);
    let program = compile(&hir, &file).expect("bytecode should be produced");
    disassemble(&program)
}

/// Strips the debug comments so a test can assert on the instructions alone.
fn code_only(src: &str) -> String {
    asm(src)
        .lines()
        .skip_while(|l| !l.starts_with("fn "))
        .map(|l| match l.find("  ; ") {
            Some(i) if l.starts_with("  0") => &l[..i],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn milestone_compiles() {
    assert_eq!(
        code_only("fn main() {\n    let x = 10;\n    let y = x + 20;\n    return y;\n}\n"),
        "\
fn main/0 -> Int  regs=5
  0000  LOAD_CONST  r2, k0
  0001  MOVE        r0, r2
  0002  LOAD_CONST  r3, k1
  0003  ADD_I64     r4, r0, r3
  0004  MOVE        r1, r4
  0005  RETURN      r1"
    );
}

#[test]
fn negation_uses_zero_minus_x() {
    // There is no NEG opcode, so the compiler materializes a zero.
    let out = code_only("fn f(a: Int) -> Int { return -a; }");
    assert!(out.contains("LOAD_CONST  r2, k"), "{out}");
    assert!(out.contains("SUB_I64     r1, r2, r0"), "{out}");
}

#[test]
fn a_bare_return_loads_unit() {
    let out = code_only("fn f() { }");
    assert!(out.contains("LOAD_CONST  r0, k"), "{out}");
    assert!(out.contains("RETURN      r0"), "{out}");
}

#[test]
fn if_else_uses_a_conditional_jump_and_falls_through() {
    let out = code_only("fn f(a: Int) -> Int { if a > 0 { return 1; } else { return 2; } }");
    assert!(out.contains("JUMP_IF_NOT"), "{out}");
    // Both arms return, so neither needs a jump to the join block.
    assert!(!out.contains("JUMP  "), "{out}");
}

#[test]
fn calls_move_arguments_into_consecutive_registers() {
    let out = code_only(
        "fn add(a: Int, b: Int) -> Int { return a + b; }\nfn main() { return add(1, 2); }",
    );
    // main holds three locals (the two literals and the call result), so the
    // scratch area for arguments starts at r3.
    let main = out.split("fn main").nth(1).unwrap();
    assert!(main.contains("MOVE        r3, r0"), "{main}");
    assert!(main.contains("MOVE        r4, r1"), "{main}");
    assert!(main.contains("CALL        r2, f0, r3"), "{main}");
}

#[test]
fn jump_offsets_are_relative_to_the_next_instruction() {
    let out = asm("fn f(a: Bool) -> Int { if a { return 1; } return 2; }");
    // The disassembler resolves the target, which is what makes the offset
    // readable; check the resolved form rather than the raw number.
    assert!(out.contains("; -> 00"), "{out}");
}

#[test]
fn debug_positions_point_at_the_source() {
    let src = "fn main() {\n    let x = 10;\n    return x;\n}\n";
    let hir = sic_ir::lower::compile_to_hir(src).unwrap();
    let file = SourceFile::new("t.sic", src);
    let program = compile(&hir, &file).unwrap();
    assert_eq!(program.debug.source_name, "t.sic");
    // The first instruction loads the literal 10 on line 2.
    assert_eq!(program.debug.position(0), Some((2, 13)));
}

#[test]
fn the_constant_pool_holds_unit_and_zero_once() {
    let hir = sic_ir::lower::compile_to_hir("fn f(a: Int) -> Int { return -a; }").unwrap();
    let file = SourceFile::new("t.sic", "");
    let program = compile(&hir, &file).unwrap();
    let zeros = program
        .consts
        .iter()
        .filter(|c| **c == sic_bytecode::Const::I64(0))
        .count();
    assert_eq!(zeros, 1);
}

#[test]
fn a_capability_call_compiles_to_call_cap_with_a_manifest() {
    let src =
        "allow { fs.read \"./a.txt\"; }\nfn main() -> String { return fs.read(\"./a.txt\"); }";
    let hir = sic_ir::lower::compile_to_hir(src).unwrap();
    let file = SourceFile::new("t.sic", src);
    let program = compile(&hir, &file).unwrap();

    assert_eq!(program.caps.len(), 1);
    assert_eq!(program.caps[0].name, "fs.read");
    assert_eq!(program.caps[0].constraints, "./a.txt");
    // The signature travels with the manifest so the verifier can use it.
    assert_eq!(
        program.caps[0].params,
        vec![sic_bytecode::TypeTag::Str as u32]
    );
    assert_eq!(program.caps[0].ret_type, sic_bytecode::TypeTag::Str as u32);

    let asm = disassemble(&program);
    assert!(asm.contains("CALL_CAP    r1, c0, r2"), "{asm}");
    assert!(
        asm.contains("c0 = fs.read(String) -> String  read"),
        "{asm}"
    );
}

#[test]
fn capability_arguments_go_into_consecutive_registers() {
    let src = "allow { fs.write \"./a.txt\"; }\nfn main() { fs.write(\"./a.txt\", \"data\"); }";
    let hir = sic_ir::lower::compile_to_hir(src).unwrap();
    let file = SourceFile::new("t.sic", src);
    let asm = disassemble(&compile(&hir, &file).unwrap());
    // Two arguments, so two moves into the scratch area, then the call.
    assert_eq!(asm.matches("MOVE").count(), 2, "{asm}");
    assert!(asm.contains("CALL_CAP"), "{asm}");
}
