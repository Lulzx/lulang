use crate::check::Type;
use crate::ir::{
    BinaryOp, BlockId, Callee, Constant, Function, Inst, InstKind, Local, LocalId, Terminator,
    ValueId,
};
use std::collections::{HashMap, HashSet};

/// Facts recovered from the lowered control-flow graph.  Keeping these facts
/// here, rather than matching source expressions in each emitter, makes the
/// optimization contract shared by every compiled backend.
#[derive(Clone, Debug, Default)]
pub struct CfgAnalysis {
    pub loops: Vec<CanonicalLoop>,
    /// Array accesses covered by a loop-entry range check.
    pub trusted_accesses: HashMap<(BlockId, usize), usize>,
}

#[derive(Clone, Debug)]
pub struct CanonicalLoop {
    pub preheader: BlockId,
    pub blocks: HashSet<BlockId>,
    pub exit: BlockId,
    pub induction: LocalId,
    pub lower: ValueId,
    pub upper: ValueId,
    pub arrays: Vec<LocalId>,
    pub reduction: Option<Reduction>,
}

#[derive(Clone, Debug)]
pub struct Reduction {
    pub accumulator: LocalId,
    pub value: ValueId,
}

/// Analyze natural loops and the canonical induction shape produced by the
/// frontend.  The analysis deliberately checks definitions, mutations and
/// dominance instead of relying on local names from the source AST.
pub fn analyze_cfg(function: &Function) -> CfgAnalysis {
    let predecessors = predecessors(function);
    let dominators = dominators(function, &predecessors);
    let definitions = value_definitions(function);
    let mut out = CfgAnalysis::default();

    for (latch, block) in function.blocks.iter().enumerate() {
        for header in successors(&block.terminator) {
            if !dominators[latch].contains(&header) {
                continue;
            }
            let mut natural = HashSet::from([header, latch as BlockId]);
            let mut work = vec![latch as BlockId];
            while let Some(node) = work.pop() {
                for &pred in &predecessors[node as usize] {
                    if natural.insert(pred) && pred != header {
                        work.push(pred);
                    }
                }
            }
            let outside: Vec<_> = predecessors[header as usize]
                .iter()
                .copied()
                .filter(|b| !natural.contains(b))
                .collect();
            let [preheader] = outside.as_slice() else {
                continue;
            };
            let Terminator::Branch {
                condition,
                then_block,
                else_block,
            } = function.blocks[header as usize].terminator
            else {
                continue;
            };
            let exit = if natural.contains(&then_block) && !natural.contains(&else_block) {
                else_block
            } else if natural.contains(&else_block) && !natural.contains(&then_block) {
                then_block
            } else {
                continue;
            };
            let Some((_, condition_inst)) = definitions.get(&condition) else {
                continue;
            };
            let InstKind::Binary {
                op: BinaryOp::Lt,
                lhs,
                rhs: upper,
            } = condition_inst.kind
            else {
                continue;
            };
            if definitions
                .get(&upper)
                .is_some_and(|(block, _)| natural.contains(block))
            {
                continue;
            }
            let Some((_, lhs_inst)) = definitions.get(&lhs) else {
                continue;
            };
            let InstKind::Load(induction) = lhs_inst.kind else {
                continue;
            };
            let Some(lower) = last_store_to(function, *preheader, induction) else {
                continue;
            };
            if !has_unit_increment(function, &natural, induction, &definitions) {
                continue;
            }

            let mut arrays = Vec::new();
            let mut access_locations = Vec::new();
            for &block_id in &natural {
                for (index, inst) in function.blocks[block_id as usize]
                    .instructions
                    .iter()
                    .enumerate()
                {
                    let pair = match inst.kind {
                        InstKind::Index { base, index }
                        | InstKind::SetIndex { base, index, .. } => {
                            array_index_pair(base, index, induction, &definitions)
                        }
                        _ => None,
                    };
                    if let Some(array) = pair {
                        if !arrays.contains(&array) {
                            arrays.push(array);
                        }
                        access_locations.push(((block_id, index), array));
                    }
                }
            }
            arrays.retain(|array| !local_invalidated(function, &natural, *array));
            access_locations.retain(|(_, array)| arrays.contains(array));
            let reduction = find_reduction(function, &natural, *preheader, induction, &definitions);
            let loop_index = out.loops.len();
            for (location, _) in access_locations {
                out.trusted_accesses.insert(location, loop_index);
            }
            out.loops.push(CanonicalLoop {
                preheader: *preheader,
                blocks: natural,
                exit,
                induction,
                lower,
                upper,
                arrays,
                reduction,
            });
        }
    }
    out
}

fn successors(terminator: &Terminator) -> Vec<BlockId> {
    match *terminator {
        Terminator::Jump(block) => vec![block],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![then_block, else_block],
        Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
    }
}

fn predecessors(function: &Function) -> Vec<Vec<BlockId>> {
    let mut out = vec![Vec::new(); function.blocks.len()];
    for (block, contents) in function.blocks.iter().enumerate() {
        for successor in successors(&contents.terminator) {
            out[successor as usize].push(block as BlockId);
        }
    }
    out
}

fn dominators(function: &Function, predecessors: &[Vec<BlockId>]) -> Vec<HashSet<BlockId>> {
    let all: HashSet<_> = (0..function.blocks.len() as BlockId).collect();
    let mut dom = vec![all; function.blocks.len()];
    dom[function.entry as usize] = HashSet::from([function.entry]);
    loop {
        let mut changed = false;
        for block in 0..function.blocks.len() as BlockId {
            if block == function.entry {
                continue;
            }
            let mut next = predecessors[block as usize]
                .first()
                .map(|p| dom[*p as usize].clone())
                .unwrap_or_default();
            for pred in predecessors[block as usize].iter().skip(1) {
                next.retain(|candidate| dom[*pred as usize].contains(candidate));
            }
            next.insert(block);
            if next != dom[block as usize] {
                dom[block as usize] = next;
                changed = true;
            }
        }
        if !changed {
            return dom;
        }
    }
}

fn value_definitions(function: &Function) -> HashMap<ValueId, (BlockId, &Inst)> {
    let mut definitions = HashMap::new();
    for (block, contents) in function.blocks.iter().enumerate() {
        for inst in &contents.instructions {
            if let Some(value) = inst.result {
                definitions.insert(value, (block as BlockId, inst));
            }
        }
    }
    definitions
}

fn last_store_to(function: &Function, block: BlockId, local: LocalId) -> Option<ValueId> {
    function.blocks[block as usize]
        .instructions
        .iter()
        .rev()
        .find_map(|inst| match inst.kind {
            InstKind::Store {
                local: target,
                value,
            } if target == local => Some(value),
            _ => None,
        })
}

fn has_unit_increment(
    function: &Function,
    blocks: &HashSet<BlockId>,
    induction: LocalId,
    defs: &HashMap<ValueId, (BlockId, &Inst)>,
) -> bool {
    blocks.iter().any(|block| {
        function.blocks[*block as usize]
            .instructions
            .iter()
            .any(|inst| {
                let InstKind::Store { local, value } = inst.kind else {
                    return false;
                };
                if local != induction {
                    return false;
                }
                let Some((_, add)) = defs.get(&value) else {
                    return false;
                };
                let InstKind::Binary {
                    op: BinaryOp::Add,
                    lhs,
                    rhs,
                } = add.kind
                else {
                    return false;
                };
                is_load_of(defs, lhs, induction) && is_i64_constant(defs, rhs, 1)
                    || is_load_of(defs, rhs, induction) && is_i64_constant(defs, lhs, 1)
            })
    })
}

fn is_load_of(defs: &HashMap<ValueId, (BlockId, &Inst)>, value: ValueId, local: LocalId) -> bool {
    defs.get(&value)
        .is_some_and(|(_, inst)| matches!(inst.kind, InstKind::Load(id) if id == local))
}

fn is_i64_constant(
    defs: &HashMap<ValueId, (BlockId, &Inst)>,
    value: ValueId,
    expected: i64,
) -> bool {
    defs.get(&value).is_some_and(
        |(_, inst)| matches!(inst.kind, InstKind::Constant(Constant::I64(v)) if v == expected),
    )
}

fn array_index_pair(
    base: ValueId,
    index: ValueId,
    induction: LocalId,
    defs: &HashMap<ValueId, (BlockId, &Inst)>,
) -> Option<LocalId> {
    if !is_load_of(defs, index, induction) {
        return None;
    }
    defs.get(&base).and_then(|(_, inst)| match inst.kind {
        InstKind::Load(local) => Some(local),
        _ => None,
    })
}

fn local_invalidated(function: &Function, blocks: &HashSet<BlockId>, local: LocalId) -> bool {
    blocks.iter().any(|block| {
        function.blocks[*block as usize]
            .instructions
            .iter()
            .any(|inst| match &inst.kind {
                InstKind::Store { local: target, .. } => *target == local,
                InstKind::Call { inout, .. } => {
                    inout.iter().flatten().any(|target| *target == local)
                }
                _ => false,
            })
    })
}

fn find_reduction(
    function: &Function,
    blocks: &HashSet<BlockId>,
    preheader: BlockId,
    induction: LocalId,
    defs: &HashMap<ValueId, (BlockId, &Inst)>,
) -> Option<Reduction> {
    for (accumulator, local) in function.locals.iter().enumerate() {
        let accumulator = accumulator as LocalId;
        if accumulator == induction
            || !local.name.contains("$tmp")
            || !matches!(local.ty, Type::I64 | Type::F64)
        {
            continue;
        }
        let initial = last_store_to(function, preheader, accumulator)?;
        let zero = defs.get(&initial).is_some_and(|(_, inst)| {
            matches!(
                inst.kind,
                InstKind::Constant(Constant::I64(0) | Constant::F64(0.0))
            )
        });
        if !zero {
            continue;
        }
        let stores: Vec<_> = blocks
            .iter()
            .flat_map(|block| {
                function.blocks[*block as usize]
                    .instructions
                    .iter()
                    .filter_map(move |inst| match inst.kind {
                        InstKind::Store { local, value } if local == accumulator => Some(value),
                        _ => None,
                    })
            })
            .collect();
        let [stored] = stores.as_slice() else {
            continue;
        };
        let Some((_, add)) = defs.get(stored) else {
            continue;
        };
        let InstKind::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } = add.kind
        else {
            continue;
        };
        let value = if is_load_of(defs, lhs, accumulator) {
            rhs
        } else if is_load_of(defs, rhs, accumulator) {
            lhs
        } else {
            continue;
        };
        return Some(Reduction { accumulator, value });
    }
    None
}

/// Inline non-recursive user calls directly in the lowered CFG.  Values and
/// locals are remapped, returns converge on a continuation block, and inout
/// copy-out becomes ordinary stores in the caller.
pub fn inline_calls(function: &Function, callees: &[Function], budget: usize) -> Function {
    let mut out = inline_calls_unordered(function, callees, budget);
    normalize_block_order(&mut out);
    out
}

/// Renumber blocks into reverse postorder from the entry.  Inlining appends
/// continuation and callee blocks out of control-flow order, but the emitters
/// walk blocks by index and require every value's defining block to precede
/// its uses; a definition dominates its uses and dominators precede what they
/// dominate in reverse postorder, so this ordering restores that contract.
/// Unreachable blocks are dropped.
fn normalize_block_order(function: &mut Function) {
    let count = function.blocks.len();
    let mut visited = vec![false; count];
    let mut postorder = Vec::with_capacity(count);
    let mut stack = vec![(function.entry, 0usize)];
    visited[function.entry as usize] = true;
    while let Some(&mut (block, ref mut next)) = stack.last_mut() {
        let successors = successors(&function.blocks[block as usize].terminator);
        if let Some(&successor) = successors.get(*next) {
            *next += 1;
            if !visited[successor as usize] {
                visited[successor as usize] = true;
                stack.push((successor, 0));
            }
        } else {
            postorder.push(block);
            stack.pop();
        }
    }
    let order: Vec<BlockId> = postorder.into_iter().rev().collect();
    if order
        .iter()
        .enumerate()
        .all(|(index, &block)| index as BlockId == block)
        && order.len() == count
    {
        return;
    }
    let mut renumbered = vec![0 as BlockId; count];
    for (index, &block) in order.iter().enumerate() {
        renumbered[block as usize] = index as BlockId;
    }
    let old_blocks = std::mem::take(&mut function.blocks);
    let mut old_blocks: Vec<Option<crate::ir::Block>> = old_blocks.into_iter().map(Some).collect();
    for &old in &order {
        let mut block = old_blocks[old as usize].take().unwrap();
        block.terminator = match block.terminator {
            Terminator::Jump(target) => Terminator::Jump(renumbered[target as usize]),
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => Terminator::Branch {
                condition,
                then_block: renumbered[then_block as usize],
                else_block: renumbered[else_block as usize],
            },
            other => other,
        };
        function.blocks.push(block);
    }
    function.entry = 0;
}

fn inline_calls_unordered(function: &Function, callees: &[Function], budget: usize) -> Function {
    let mut out = function.clone();
    let recursive = recursive_functions(callees);
    let mut spent = 0;
    loop {
        let mut changed = false;
        'site: for block in 0..out.blocks.len() {
            for index in 0..out.blocks[block].instructions.len() {
                let InstKind::Call {
                    callee: Callee::Function(id),
                    ref args,
                    ref inout,
                } = out.blocks[block].instructions[index].kind
                else {
                    continue;
                };
                let id = id as usize;
                if id >= callees.len() || recursive.contains(&(id as u32)) {
                    continue;
                }
                let size: usize = callees[id]
                    .blocks
                    .iter()
                    .map(|block| block.instructions.len() + 1)
                    .sum();
                if spent + size > budget {
                    return out;
                }
                let args = args.clone();
                let inout = inout.clone();
                inline_site(
                    &mut out,
                    block as BlockId,
                    index,
                    &callees[id],
                    &args,
                    &inout,
                );
                spent += size;
                changed = true;
                break 'site;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

fn recursive_functions(functions: &[Function]) -> HashSet<u32> {
    fn reaches(start: u32, current: u32, functions: &[Function], seen: &mut HashSet<u32>) -> bool {
        if !seen.insert(current) {
            return false;
        }
        functions
            .get(current as usize)
            .into_iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .filter_map(|inst| match inst.kind {
                InstKind::Call {
                    callee: Callee::Function(id),
                    ..
                } => Some(id),
                _ => None,
            })
            .any(|next| next == start || reaches(start, next, functions, seen))
    }
    (0..functions.len() as u32)
        .filter(|&id| reaches(id, id, functions, &mut HashSet::new()))
        .collect()
}

fn inline_site(
    caller: &mut Function,
    call_block: BlockId,
    call_index: usize,
    callee: &Function,
    args: &[ValueId],
    inout: &[Option<LocalId>],
) {
    let call = caller.blocks[call_block as usize].instructions[call_index].clone();
    let result = call.result;
    let result_local = caller.locals.len() as LocalId;
    caller.locals.push(Local {
        name: format!("$inline_result{}", result_local),
        ty: call.ty.clone(),
        mutable: true,
    });

    let continuation = caller.blocks.len() as BlockId;
    let tail = caller.blocks[call_block as usize]
        .instructions
        .split_off(call_index + 1);
    let old_terminator = std::mem::replace(
        &mut caller.blocks[call_block as usize].terminator,
        Terminator::Unreachable,
    );
    caller.blocks[call_block as usize].instructions.pop();
    let mut continuation_instructions = Vec::new();
    if let Some(value) = result {
        continuation_instructions.push(Inst {
            result: Some(value),
            ty: call.ty.clone(),
            kind: InstKind::Load(result_local),
        });
    }
    continuation_instructions.extend(tail);
    caller.blocks.push(crate::ir::Block {
        instructions: continuation_instructions,
        terminator: old_terminator,
    });

    let local_base = caller.locals.len() as LocalId;
    caller
        .locals
        .extend(callee.locals.iter().cloned().map(|mut local| {
            local.name = format!("$inlined{}_{}", local_base, local.name);
            local
        }));
    let map_local = |local: LocalId| local_base + local;
    for (&param, &argument) in callee.params.iter().zip(args) {
        caller.blocks[call_block as usize].instructions.push(Inst {
            result: None,
            ty: Type::Unit,
            kind: InstKind::Store {
                local: map_local(param),
                value: argument,
            },
        });
    }

    let block_base = caller.blocks.len() as BlockId;
    let map_block = |block: BlockId| block_base + block;
    let value_base = caller.values.len() as ValueId;
    caller.values.extend(callee.values.iter().cloned());
    let map_value = |value: ValueId| value_base + value;
    caller.blocks[call_block as usize].terminator = Terminator::Jump(map_block(callee.entry));

    for source in &callee.blocks {
        let mut instructions = Vec::with_capacity(source.instructions.len() + inout.len() * 2 + 1);
        for inst in &source.instructions {
            instructions.push(remap_inst(inst, map_local, map_value));
        }
        let terminator = match source.terminator {
            Terminator::Jump(block) => Terminator::Jump(map_block(block)),
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => Terminator::Branch {
                condition: map_value(condition),
                then_block: map_block(then_block),
                else_block: map_block(else_block),
            },
            Terminator::Return(value) => {
                instructions.push(Inst {
                    result: None,
                    ty: Type::Unit,
                    kind: InstKind::Store {
                        local: result_local,
                        value: map_value(value),
                    },
                });
                for (index, target) in inout.iter().enumerate() {
                    let Some(target) = target else { continue };
                    let param = callee.params[index];
                    let ty = callee.locals[param as usize].ty.clone();
                    let loaded = caller.values.len() as ValueId;
                    caller.values.push(ty.clone());
                    instructions.push(Inst {
                        result: Some(loaded),
                        ty,
                        kind: InstKind::Load(map_local(param)),
                    });
                    instructions.push(Inst {
                        result: None,
                        ty: Type::Unit,
                        kind: InstKind::Store {
                            local: *target,
                            value: loaded,
                        },
                    });
                }
                Terminator::Jump(continuation)
            }
            Terminator::Unreachable => Terminator::Unreachable,
        };
        caller.blocks.push(crate::ir::Block {
            instructions,
            terminator,
        });
    }
}

fn remap_inst(
    inst: &Inst,
    local: impl Fn(LocalId) -> LocalId,
    value: impl Fn(ValueId) -> ValueId,
) -> Inst {
    let kind = match &inst.kind {
        InstKind::Constant(constant) => InstKind::Constant(constant.clone()),
        InstKind::Load(id) => InstKind::Load(local(*id)),
        InstKind::Store {
            local: id,
            value: v,
        } => InstKind::Store {
            local: local(*id),
            value: value(*v),
        },
        InstKind::Unary { op, value: v } => InstKind::Unary {
            op: *op,
            value: value(*v),
        },
        InstKind::Binary { op, lhs, rhs } => InstKind::Binary {
            op: *op,
            lhs: value(*lhs),
            rhs: value(*rhs),
        },
        InstKind::Select {
            condition,
            then_value,
            else_value,
        } => InstKind::Select {
            condition: value(*condition),
            then_value: value(*then_value),
            else_value: value(*else_value),
        },
        InstKind::Call {
            callee,
            args,
            inout,
        } => InstKind::Call {
            callee: callee.clone(),
            args: args.iter().map(|v| value(*v)).collect(),
            inout: inout.iter().map(|id| id.map(&local)).collect(),
        },
        InstKind::Field {
            base,
            record,
            field,
        } => InstKind::Field {
            base: value(*base),
            record: *record,
            field: *field,
        },
        InstKind::Index { base, index } => InstKind::Index {
            base: value(*base),
            index: value(*index),
        },
        InstKind::Array(items) => InstKind::Array(items.iter().map(|v| value(*v)).collect()),
        InstKind::Record { record, fields } => InstKind::Record {
            record: *record,
            fields: fields.iter().map(|v| value(*v)).collect(),
        },
        InstKind::Enum { enumeration, tag } => InstKind::Enum {
            enumeration: *enumeration,
            tag: *tag,
        },
        InstKind::SetIndex {
            root,
            path,
            base,
            index,
            value: stored,
        } => InstKind::SetIndex {
            root: local(*root),
            path: path.clone(),
            base: value(*base),
            index: value(*index),
            value: value(*stored),
        },
        InstKind::SetField {
            root,
            path,
            value: stored,
        } => InstKind::SetField {
            root: local(*root),
            path: path.clone(),
            value: value(*stored),
        },
    };
    Inst {
        result: inst.result.map(value),
        ty: inst.ty.clone(),
        kind,
    }
}

/// Convert safe, single-block CFG diamonds into straight-line selects.  This
/// exposes invariant values to GVN/LICM while retaining trapping and I/O order.
pub fn if_convert(function: &mut Function) {
    let original_len = function.blocks.len();
    let predecessors = predecessors(function);
    for branch in 0..original_len {
        let Terminator::Branch {
            condition,
            then_block,
            else_block,
        } = function.blocks[branch].terminator
        else {
            continue;
        };
        let (then_insts, then_jump) = {
            let block = &function.blocks[then_block as usize];
            (block.instructions.clone(), block.terminator.clone())
        };
        let (else_insts, else_jump) = {
            let block = &function.blocks[else_block as usize];
            (block.instructions.clone(), block.terminator.clone())
        };
        let (Terminator::Jump(then_merge), Terminator::Jump(else_merge)) = (then_jump, else_jump)
        else {
            continue;
        };
        if then_merge != else_merge
            || predecessors[then_block as usize] != [branch as BlockId]
            || predecessors[else_block as usize] != [branch as BlockId]
            || !speculatable_arm(&then_insts, function)
            || !speculatable_arm(&else_insts, function)
        {
            continue;
        }
        let mut then_stores = HashMap::new();
        let mut else_stores = HashMap::new();
        let mut moved = Vec::new();
        for inst in then_insts {
            match inst.kind {
                InstKind::Store { local, value } => {
                    then_stores.insert(local, value);
                }
                _ => moved.push(inst),
            }
        }
        for inst in else_insts {
            match inst.kind {
                InstKind::Store { local, value } => {
                    else_stores.insert(local, value);
                }
                _ => moved.push(inst),
            }
        }
        let locals: HashSet<_> = then_stores
            .keys()
            .chain(else_stores.keys())
            .copied()
            .collect();
        if locals.iter().any(|local| {
            then_stores
                .get(local)
                .into_iter()
                .chain(else_stores.get(local))
                .any(|value| function.values[*value as usize] != function.locals[*local as usize].ty)
        }) {
            continue;
        }
        for local in locals {
            let prior = if then_stores.contains_key(&local) && else_stores.contains_key(&local) {
                None
            } else {
                let id = function.values.len() as ValueId;
                let ty = function.locals[local as usize].ty.clone();
                function.values.push(ty.clone());
                moved.push(Inst {
                    result: Some(id),
                    ty,
                    kind: InstKind::Load(local),
                });
                Some(id)
            };
            let yes = then_stores.get(&local).copied().or(prior).unwrap();
            let no = else_stores.get(&local).copied().or(prior).unwrap();
            let selected = function.values.len() as ValueId;
            let ty = function.locals[local as usize].ty.clone();
            function.values.push(ty.clone());
            moved.push(Inst {
                result: Some(selected),
                ty,
                kind: InstKind::Select {
                    condition,
                    then_value: yes,
                    else_value: no,
                },
            });
            moved.push(Inst {
                result: None,
                ty: Type::Unit,
                kind: InstKind::Store {
                    local,
                    value: selected,
                },
            });
        }
        function.blocks[branch].instructions.extend(moved);
        function.blocks[branch].terminator = Terminator::Jump(then_merge);
        function.blocks[then_block as usize].instructions.clear();
        function.blocks[then_block as usize].terminator = Terminator::Unreachable;
        function.blocks[else_block as usize].instructions.clear();
        function.blocks[else_block as usize].terminator = Terminator::Unreachable;
    }
}

fn speculatable_arm(instructions: &[Inst], function: &Function) -> bool {
    let mut stored = HashSet::new();
    instructions.iter().all(|inst| match &inst.kind {
        InstKind::Constant(_)
        | InstKind::Unary { .. }
        | InstKind::Field { .. }
        | InstKind::Record { .. }
        | InstKind::Enum { .. }
        | InstKind::Select { .. } => true,
        InstKind::Load(local) => !stored.contains(local),
        InstKind::Store { local, .. } => stored.insert(*local),
        InstKind::Binary { op, lhs, rhs } => {
            !matches!(op, BinaryOp::Div | BinaryOp::Rem)
                || function.values[*lhs as usize] == Type::F64
                || function.values[*rhs as usize] == Type::F64
        }
        InstKind::Call {
            callee: Callee::Builtin(name),
            inout,
            ..
        } => {
            inout.iter().all(Option::is_none)
                && matches!(
                    name.as_str(),
                    "sqrt"
                        | "abs"
                        | "floor"
                        | "sin"
                        | "cos"
                        | "acos"
                        | "min"
                        | "max"
                        | "pow"
                        | "atan2"
                        | "float"
                        | "int"
                        | "len"
                )
        }
        InstKind::Call { .. }
        | InstKind::Index { .. }
        | InstKind::Array(_)
        | InstKind::SetIndex { .. }
        | InstKind::SetField { .. } => false,
    })
}

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

#[cfg(test)]
mod cfg_tests {
    use super::*;

    fn lower(source: &str) -> crate::ir::LoweredProgram {
        let tokens = crate::lexer::lex(source).unwrap();
        let mut parser = crate::parser::Parser::new(tokens);
        parser.parse().unwrap();
        crate::ir::LoweredProgram::lower(parser.prog).unwrap()
    }

    #[test]
    fn finds_reduction_and_trusted_array_accesses_from_cfg() {
        let function = lower(
            "main { let a = arr(8, 1.0) print(sum(i in 0..len(a)) a[i] * 2.0) }",
        )
        .main
        .unwrap();
        let analysis = analyze_cfg(&function);
        assert_eq!(analysis.loops.len(), 1, "{analysis:#?}");
        assert!(analysis.loops[0].reduction.is_some(), "{analysis:#?}");
        assert_eq!(analysis.loops[0].arrays.len(), 1, "{analysis:#?}");
        assert!(!analysis.trusted_accesses.is_empty(), "{analysis:#?}");
    }

    #[test]
    fn cfg_if_conversion_uses_selects_only_for_safe_diamonds() {
        let mut function = lower(
            "main { var x = 1.0 if true { x = -x } else { x = x + 2.0 } print(x) }",
        )
        .main
        .unwrap();
        if_convert(&mut function);
        assert!(function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|inst| matches!(inst.kind, InstKind::Select { .. })));
    }

    #[test]
    fn cfg_inlining_exposes_callee_reductions_to_loop_analysis() {
        let program = lower(
            "fn dot(a: [f64], n: i64): f64 { sum(i in 0..n) a[i] }\n\
             main { let a = arr(8, 1.0) print(dot(a, len(a))) }",
        );
        let main = inline_calls(program.main.as_ref().unwrap(), &program.functions, 3000);
        assert!(!main
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|inst| matches!(
                inst.kind,
                InstKind::Call {
                    callee: Callee::Function(_),
                    ..
                }
            )));
        assert!(analyze_cfg(&main)
            .loops
            .iter()
            .any(|loop_info| loop_info.reduction.is_some()));
    }
}
