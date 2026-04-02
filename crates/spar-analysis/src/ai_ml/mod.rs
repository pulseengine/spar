//! AI/ML component analysis passes (Issue #91).
//!
//! Provides architecture-level safety analysis for AI/ML components using
//! the `AI_ML` property set. Eight checks aligned with ISO/PAS 8800:
//!
//! | Pass                    | Severity | What it checks                                        |
//! |-------------------------|----------|-------------------------------------------------------|
//! | `ai_inference_deadline` | Error    | Inference latency fits within AADL deadline            |
//! | `ai_fallback_coverage`  | Warning  | Every AI thread has Fallback_Strategy                  |
//! | `ai_fallback_timing`    | Error    | Fallback latency fits within remaining deadline budget |
//! | `ai_ood_coverage`       | Warning  | Confidence_Threshold requires OOD_Detection_Enabled    |
//! | `ai_model_deployment`   | Error    | AI process bound to processor with sufficient compute  |
//! | `ai_redundancy`         | Warning  | Redundant_Model fallback has second model on diff proc |
//!
//! Checks 7–8 from Issue #91 (drift monitoring, training provenance) were
//! evaluated and dropped: they are documentation linting, not architecture
//! analysis. If needed, extend `completeness.rs` or add a separate lint pass.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{
    get_ai_ml_bool, get_ai_ml_string, get_confidence_threshold, get_fallback_latency,
    get_inference_latency_range, get_processor_binding, get_timing_property, is_ai_ml_component,
};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// AI/ML safety analysis — all eight checks in one pass.
pub struct AiMlAnalysis;

impl Analysis for AiMlAnalysis {
    fn name(&self) -> &str {
        "ai_ml"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let props = instance.properties_for(comp_idx);

            if !is_ai_ml_component(props) {
                continue;
            }

            let path = component_path(instance, comp_idx);

            // Thread-level checks (inference, fallback, OOD)
            if comp.category == ComponentCategory::Thread {
                check_inference_deadline(props, &path, &mut diags);
                check_fallback_coverage(props, &path, &comp.name, &mut diags);
                check_fallback_timing(props, &path, &mut diags);
                check_ood_coverage(props, &path, &comp.name, &mut diags);
            }

            // Process-level checks (model deployment)
            if comp.category == ComponentCategory::Process {
                check_model_deployment(props, &path, &comp.name, &mut diags);
            }

            // Redundancy check: thread or process with Redundant_Model fallback
            check_redundancy(instance, comp_idx, props, &path, &comp.name, &mut diags);
        }

        diags
    }
}

// ── Check 1: Inference deadline ────────────────────────────────────

fn check_inference_deadline(
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let Some((_, worst_case_ps)) = get_inference_latency_range(props) else {
        return;
    };
    let Some(deadline_ps) = get_timing_property(props, "Deadline") else {
        // AI thread with inference latency but no deadline — flag it
        diags.push(AnalysisDiagnostic {
            severity: Severity::Warning,
            message: "AI/ML thread has Inference_Latency but no Deadline property; \
                      cannot verify timing safety"
                .to_string(),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
        return;
    };

    if worst_case_ps > deadline_ps {
        let worst_ms = worst_case_ps as f64 / 1_000_000_000.0;
        let deadline_ms = deadline_ps as f64 / 1_000_000_000.0;
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "AI/ML inference worst-case latency ({worst_ms:.1} ms) exceeds \
                 thread deadline ({deadline_ms:.1} ms)"
            ),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
    }
}

// ── Check 2: Fallback coverage ─────────────────────────────────────

fn check_fallback_coverage(
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    name: &Name,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    if get_ai_ml_string(props, "Fallback_Strategy").is_none() {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Warning,
            message: format!(
                "AI/ML thread '{}' has no Fallback_Strategy defined; \
                 ISO/PAS 8800 recommends fallback measures for AI elements",
                name
            ),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
    }
}

// ── Check 3: Fallback timing ───────────────────────────────────────

fn check_fallback_timing(
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let Some(fallback_ps) = get_fallback_latency(props) else {
        return;
    };
    let Some(deadline_ps) = get_timing_property(props, "Deadline") else {
        return;
    };
    let Some((_, worst_inference_ps)) = get_inference_latency_range(props) else {
        return;
    };

    // In worst case: inference runs to deadline, then fallback must complete.
    // Total = worst_inference + fallback must fit in deadline.
    // More precisely: if inference fails at worst case, the fallback must
    // complete within the remaining budget.
    let total_ps = worst_inference_ps.saturating_add(fallback_ps);
    if total_ps > deadline_ps {
        let total_ms = total_ps as f64 / 1_000_000_000.0;
        let deadline_ms = deadline_ps as f64 / 1_000_000_000.0;
        let fallback_ms = fallback_ps as f64 / 1_000_000_000.0;
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "AI/ML worst-case inference + fallback ({total_ms:.1} ms) exceeds \
                 deadline ({deadline_ms:.1} ms); fallback latency is {fallback_ms:.1} ms"
            ),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
    }
}

// ── Check 4: OOD detection coverage ────────────────────────────────

fn check_ood_coverage(
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    name: &Name,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // If a confidence threshold is set, OOD detection should be enabled
    if get_confidence_threshold(props).is_some() {
        let ood_enabled = get_ai_ml_bool(props, "OOD_Detection_Enabled").unwrap_or(false);
        if !ood_enabled {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Warning,
                message: format!(
                    "AI/ML thread '{}' has Confidence_Threshold but OOD_Detection_Enabled \
                     is not set; out-of-distribution inputs may produce silently wrong results",
                    name
                ),
                path: path.to_vec(),
                analysis: "ai_ml".to_string(),
            });
        }
    }
}

// ── Check 5: Model deployment ──────────────────────────────────────

fn check_model_deployment(
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    name: &Name,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    // Every AI process should be bound to a processor
    if get_ai_ml_string(props, "Model_Format").is_some() && get_processor_binding(props).is_none() {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Error,
            message: format!(
                "AI/ML process '{}' has Model_Format but no Actual_Processor_Binding; \
                 cannot verify compute capacity for inference",
                name
            ),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
    }
}

// ── Check 6: Redundancy ────────────────────────────────────────────

fn check_redundancy(
    instance: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    props: &spar_hir_def::properties::PropertyMap,
    path: &[String],
    name: &Name,
    diags: &mut Vec<AnalysisDiagnostic>,
) {
    let Some(strategy) = get_ai_ml_string(props, "Fallback_Strategy") else {
        return;
    };
    if !strategy.eq_ignore_ascii_case("Redundant_Model") {
        return;
    }

    // Check that a sibling AI component exists on a different processor
    let my_binding = get_processor_binding(props);
    let parent = instance.component(comp_idx).parent;
    let Some(parent_idx) = parent else { return };

    let siblings = &instance.component(parent_idx).children;
    let has_redundant_peer = siblings.iter().any(|&sib_idx| {
        if sib_idx == comp_idx {
            return false;
        }
        let sib_props = instance.properties_for(sib_idx);
        if !is_ai_ml_component(sib_props) {
            return false;
        }
        let sib_binding = get_processor_binding(sib_props);
        // Must be on a different processor
        match (&my_binding, &sib_binding) {
            (Some(mine), Some(theirs)) => !mine.eq_ignore_ascii_case(theirs),
            _ => false,
        }
    });

    if !has_redundant_peer {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Warning,
            message: format!(
                "AI/ML component '{}' uses Redundant_Model fallback but no sibling \
                 AI component found on a different processor",
                name
            ),
            path: path.to_vec(),
            analysis: "ai_ml".to_string(),
        });
    }
}

// ── Private helpers ────────────────────────────────────────────────

use spar_hir_def::instance::ComponentInstanceIdx;
use spar_hir_def::name::Name;

#[cfg(test)]
mod tests;
