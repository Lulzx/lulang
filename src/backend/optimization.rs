use crate::ast::{Expr, ExprId, Program, Stmt, StmtId};

/// Cranelift loop-invariant code motion for pure, non-trapping instructions and
/// explicitly whitelisted math imports.
pub fn licm(
    func: &mut cranelift_codegen::ir::Function,
    pure_imports: &std::collections::HashSet<u32>,
) {
    use cranelift_codegen::dominator_tree::DominatorTree;
    use cranelift_codegen::flowgraph::ControlFlowGraph;
    use cranelift_codegen::ir::Block;
    use cranelift_codegen::loop_analysis::LoopAnalysis;

    let pure_call = |func: &cranelift_codegen::ir::Function, inst: cranelift_codegen::ir::Inst| {
        use cranelift_codegen::ir::{ExternalName, InstructionData};
        let InstructionData::Call { func_ref, .. } = func.dfg.insts[inst] else {
            return false;
        };
        match func.dfg.ext_funcs[func_ref].name {
            ExternalName::User(reference) => func
                .params
                .user_named_funcs()
                .get(reference)
                .is_some_and(|name| name.namespace == 0 && pure_imports.contains(&name.index)),
            _ => false,
        }
    };

    let cfg = ControlFlowGraph::with_function(func);
    let domtree = DominatorTree::with_function(func, &cfg);
    let mut loops = LoopAnalysis::new();
    loops.compute(func, &cfg, &domtree);

    for lp in loops.loops().collect::<Vec<_>>() {
        let header = loops.loop_header(lp);
        let outside: Vec<Block> = cfg
            .pred_iter(header)
            .map(|pred| pred.block)
            .filter(|block| !loops.is_in_loop(*block, lp))
            .collect();
        let [preheader] = outside[..] else { continue };
        if loops.loop_level(preheader).level() >= loops.loop_level(header).level() {
            continue;
        }
        let Some(terminator) = func.layout.last_inst(preheader) else {
            continue;
        };
        let body: Vec<Block> = func
            .layout
            .blocks()
            .filter(|block| loops.is_in_loop(*block, lp))
            .collect();
        loop {
            let mut changed = false;
            for &block in &body {
                let mut next = func.layout.first_inst(block);
                while let Some(inst) = next {
                    next = func.layout.next_inst(inst);
                    let op = func.dfg.insts[inst].opcode();
                    if (op.is_branch()
                        || op.is_call()
                        || op.is_return()
                        || op.is_terminator()
                        || op.can_load()
                        || op.can_store()
                        || op.can_trap()
                        || op.other_side_effects())
                        && !(op.is_call() && pure_call(func, inst))
                    {
                        continue;
                    }
                    let invariant = func.dfg.inst_args(inst).to_vec().into_iter().all(|value| {
                        let value = func.dfg.resolve_aliases(value);
                        let block = match func.dfg.value_def(value) {
                            cranelift_codegen::ir::ValueDef::Result(def, _) => {
                                func.layout.inst_block(def)
                            }
                            cranelift_codegen::ir::ValueDef::Param(def, _) => Some(def),
                            _ => None,
                        };
                        block.is_some_and(|block| !loops.is_in_loop(block, lp))
                    });
                    if invariant {
                        func.layout.remove_inst(inst);
                        func.layout.insert_inst(inst, terminator);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
}

/// Find arrays indexed as `a[i]` in a loop and reject bodies that can invalidate
/// the hoisted check by shadowing the induction variable or rebinding an array.
pub fn scan_trusted_expr(
    p: &Program,
    expr: ExprId,
    var: &str,
    arrays: &mut Vec<String>,
    valid: &mut bool,
) {
    walk_expr(p, expr, var, arrays, valid)
}

pub fn scan_trusted(p: &Program, statements: &[StmtId], var: &str) -> Option<Vec<String>> {
    let mut arrays = Vec::new();
    let mut valid = true;
    walk_statements(p, statements, var, &mut arrays, &mut valid);
    (valid && !arrays.is_empty()).then_some(arrays)
}

fn walk_expr(p: &Program, expr: ExprId, var: &str, arrays: &mut Vec<String>, valid: &mut bool) {
    match p.expr(expr) {
        Expr::Index(array, index) => {
            if let (Expr::Ident(array_name), Expr::Ident(index_name)) =
                (p.expr(*array), p.expr(*index))
            {
                if index_name == var && !arrays.contains(array_name) {
                    arrays.push(array_name.clone());
                }
            }
            walk_expr(p, *array, var, arrays, valid);
            walk_expr(p, *index, var, arrays, valid);
        }
        Expr::Bin(_, left, right) => {
            walk_expr(p, *left, var, arrays, valid);
            walk_expr(p, *right, var, arrays, valid);
        }
        Expr::Un(_, value) | Expr::Circum(_, value) | Expr::Field(value, _) => {
            walk_expr(p, *value, var, arrays, valid)
        }
        Expr::Call(name, args) => {
            if p.find_fn(name).is_some_and(|function| function.has_inout()) {
                *valid = false;
            }
            args.iter()
                .for_each(|&arg| walk_expr(p, arg, var, arrays, valid));
        }
        Expr::Array(items) => items
            .iter()
            .for_each(|&item| walk_expr(p, item, var, arrays, valid)),
        Expr::Record(_, fields) => fields
            .iter()
            .for_each(|(_, value)| walk_expr(p, *value, var, arrays, valid)),
        Expr::Sum {
            var: nested,
            lo,
            hi,
            body,
        } => {
            walk_expr(p, *lo, var, arrays, valid);
            walk_expr(p, *hi, var, arrays, valid);
            if nested == var {
                *valid = false;
            } else {
                walk_expr(p, *body, var, arrays, valid);
            }
        }
        _ => {}
    }
}

fn walk_statements(
    p: &Program,
    statements: &[StmtId],
    var: &str,
    arrays: &mut Vec<String>,
    valid: &mut bool,
) {
    for &statement in statements {
        match p.stmt(statement) {
            Stmt::Let(name, expr) | Stmt::Var(name, expr) => {
                if name == var {
                    *valid = false;
                }
                walk_expr(p, *expr, var, arrays, valid);
            }
            Stmt::Assign(target, value) => {
                if matches!(p.expr(*target), Expr::Ident(name) if arrays.contains(name)) {
                    *valid = false;
                }
                walk_expr(p, *target, var, arrays, valid);
                walk_expr(p, *value, var, arrays, valid);
            }
            Stmt::If(condition, then_body, else_body) => {
                walk_expr(p, *condition, var, arrays, valid);
                walk_statements(p, then_body, var, arrays, valid);
                walk_statements(p, else_body, var, arrays, valid);
            }
            Stmt::For(nested, lo, hi, body) => {
                walk_expr(p, *lo, var, arrays, valid);
                walk_expr(p, *hi, var, arrays, valid);
                if nested == var {
                    *valid = false;
                } else {
                    walk_statements(p, body, var, arrays, valid);
                }
            }
            Stmt::While(condition, body) => {
                walk_expr(p, *condition, var, arrays, valid);
                walk_statements(p, body, var, arrays, valid);
            }
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => walk_expr(p, *expr, var, arrays, valid),
            Stmt::Return(None) => {}
        }
    }
}
