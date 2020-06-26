use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{self, BasicBlock, Location};
use rustc_middle::ty::TyCtxt;
use rustc_span::Span;

use super::ops;
use super::qualifs::{NeedsDrop, Qualif};
use super::validation::Qualifs;
use super::ConstCx;

/// Returns `true` if we should use the more precise live drop checker that runs after drop
/// elaboration.
pub fn checking_enabled(tcx: TyCtxt<'tcx>) -> bool {
    tcx.features().const_precise_live_drops
}

/// Look for live drops in a const context.
///
/// This is separate from the rest of the const checking logic because it must run after drop
/// elaboration.
pub fn check_live_drops(tcx: TyCtxt<'tcx>, def_id: LocalDefId, body: &mir::Body<'tcx>) {
    let const_kind = tcx.hir().body_const_context(def_id);
    if const_kind.is_none() {
        return;
    }

    if !checking_enabled(tcx) {
        return;
    }

    let ccx = ConstCx {
        body,
        tcx,
        def_id: def_id.to_def_id(),
        const_kind,
        param_env: tcx.param_env(def_id),
    };

    let mut visitor = CheckLiveDrops { ccx: &ccx, qualifs: Qualifs::default() };

    visitor.visit_body(body);
}

struct CheckLiveDrops<'mir, 'tcx> {
    ccx: &'mir ConstCx<'mir, 'tcx>,
    qualifs: Qualifs<'mir, 'tcx>,
}

// So we can access `body` and `tcx`.
impl std::ops::Deref for CheckLiveDrops<'mir, 'tcx> {
    type Target = ConstCx<'mir, 'tcx>;

    fn deref(&self) -> &Self::Target {
        &self.ccx
    }
}

impl CheckLiveDrops<'mir, 'tcx> {
    fn check_live_drop(&self, span: Span) {
        ops::non_const(self.ccx, ops::LiveDrop(None), span);
    }
}

impl Visitor<'tcx> for CheckLiveDrops<'mir, 'tcx> {
    fn visit_basic_block_data(&mut self, bb: BasicBlock, block: &mir::BasicBlockData<'tcx>) {
        trace!("visit_basic_block_data: bb={:?} is_cleanup={:?}", bb, block.is_cleanup);

        // Ignore drop terminators in cleanup blocks.
        if block.is_cleanup {
            return;
        }

        self.super_basic_block_data(bb, block);
    }

    fn visit_terminator(&mut self, terminator: &mir::Terminator<'tcx>, location: Location) {
        trace!("visit_terminator: terminator={:?} location={:?}", terminator, location);

        match &terminator.kind {
            mir::TerminatorKind::Drop { place: dropped_place, .. } => {
                let dropped_ty = dropped_place.ty(self.body, self.tcx).ty;
                if !NeedsDrop::in_any_value_of_ty(self.ccx, dropped_ty) {
                    return;
                }

                if dropped_place.is_indirect() {
                    self.check_live_drop(terminator.source_info.span);
                    return;
                }

                if self.qualifs.needs_drop(self.ccx, dropped_place.local, location) {
                    // Use the span where the dropped local was declared for the error.
                    let span = self.body.local_decls[dropped_place.local].source_info.span;
                    self.check_live_drop(span);
                }
            }

            mir::TerminatorKind::DropAndReplace { .. } => span_bug!(
                terminator.source_info.span,
                "`DropAndReplace` should be removed by drop elaboration",
            ),

            mir::TerminatorKind::Abort
            | mir::TerminatorKind::Call { .. }
            | mir::TerminatorKind::Assert { .. }
            | mir::TerminatorKind::FalseEdge { .. }
            | mir::TerminatorKind::FalseUnwind { .. }
            | mir::TerminatorKind::GeneratorDrop
            | mir::TerminatorKind::Goto { .. }
            | mir::TerminatorKind::InlineAsm { .. }
            | mir::TerminatorKind::Resume
            | mir::TerminatorKind::Return
            | mir::TerminatorKind::SwitchInt { .. }
            | mir::TerminatorKind::Unreachable
            | mir::TerminatorKind::Yield { .. } => {}
        }
    }
}
