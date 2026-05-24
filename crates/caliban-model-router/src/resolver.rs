//! Pure resolution pipeline: request → ordered candidate routes.
//!
//! Resolution is separated from dispatch so tests can pin candidate ordering
//! without spinning up a mock provider's `complete()` path.

use std::collections::HashMap;
use std::sync::Arc;

use caliban_provider::{CompletionRequest, Provider, RequestPurpose};

use crate::breaker::CircuitBreaker;
use crate::capabilities::{
    CandidateAnnotation, CandidateOrigin, DerivedNeeds, caps_satisfy_needs,
    caps_satisfy_route_requires, route_requires_consistent_with_needs,
};
use crate::config::RouteEntry;
use crate::error::{Result, RouterError};

/// One candidate produced by [`resolve_candidates`].
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Index into the router's `routes` vec.
    pub route_idx: usize,
    /// Annotation explaining why this candidate was kept.
    pub annotation: CandidateAnnotation,
}

/// Per-candidate diagnostic record — produced by [`resolve_with_diagnostics`].
#[derive(Debug, Clone)]
pub struct DiagnosticEntry {
    /// Route id.
    pub route_id: String,
    /// Whether the route was kept.
    pub kept: bool,
    /// Reason it was kept or dropped.
    pub reason: String,
    /// Origin label when kept.
    pub origin: Option<CandidateOrigin>,
    /// Current breaker state name.
    pub breaker_state: &'static str,
}

/// Resolve candidate routes for `request`. The returned vec is in dispatch
/// order: `[primary, fallback_1, fallback_2, ...]`.
///
/// # Errors
/// Returns [`RouterError::NoCandidate`] when no route satisfies both the
/// purpose and the derived capability needs, or [`RouterError::UnknownFallbackId`]
/// if an explicit `fallback = [...]` references a missing route id.
pub fn resolve_candidates(
    routes: &[RouteEntry],
    breakers: &HashMap<String, CircuitBreaker>,
    providers: &HashMap<String, Arc<dyn Provider + Send + Sync>>,
    default_purpose: RequestPurpose,
    request: &CompletionRequest,
) -> Result<Vec<Candidate>> {
    let (cands, _) =
        resolve_with_diagnostics(routes, breakers, providers, default_purpose, request)?;
    Ok(cands)
}

/// Resolve candidates *and* return per-route diagnostics for `caliban router
/// debug` overlays.
///
/// # Errors
/// See [`resolve_candidates`].
pub fn resolve_with_diagnostics(
    routes: &[RouteEntry],
    breakers: &HashMap<String, CircuitBreaker>,
    providers: &HashMap<String, Arc<dyn Provider + Send + Sync>>,
    default_purpose: RequestPurpose,
    request: &CompletionRequest,
) -> Result<(Vec<Candidate>, Vec<DiagnosticEntry>)> {
    let purpose = request.metadata.purpose.unwrap_or(default_purpose);
    let needs = DerivedNeeds::from_request(request);

    let mut diagnostics: Vec<DiagnosticEntry> = Vec::with_capacity(routes.len());

    // First pass: find the primary (first same-purpose, capability-compatible,
    // not Tripped) and remember each route's "viability" verdict.
    let mut viability: Vec<Option<&str>> = vec![None; routes.len()]; // None = viable; Some = reason it was dropped
    let mut breaker_states: Vec<&'static str> = vec!["closed"; routes.len()];

    for (i, r) in routes.iter().enumerate() {
        let bstate = breakers.get(&r.id).map_or("closed", |b| b.state().name());
        breaker_states[i] = bstate;
        if bstate == "tripped" {
            viability[i] = Some("breaker tripped");
            continue;
        }
        let provider = match providers.get(&r.provider) {
            Some(p) => p,
            None => {
                viability[i] = Some("provider not registered");
                continue;
            }
        };
        let caps = provider.capabilities(&r.model);
        if !caps_satisfy_needs(caps, needs) {
            viability[i] = Some("capability mismatch (request needs)");
            continue;
        }
        if !caps_satisfy_route_requires(caps, r.requires) {
            viability[i] = Some("capability mismatch (route requires)");
            continue;
        }
        if !route_requires_consistent_with_needs(r.requires, needs) {
            viability[i] = Some("route requires unmet by request");
            continue;
        }
        // viable
    }

    // Find the primary: first same-purpose, viable route.
    let primary_idx = routes
        .iter()
        .enumerate()
        .find(|(i, r)| r.purpose == purpose && viability[*i].is_none())
        .map(|(i, _)| i);

    let Some(primary_idx) = primary_idx else {
        // Build diagnostics + return NoCandidate.
        for (i, r) in routes.iter().enumerate() {
            let kept = false;
            let reason = viability[i].map_or_else(
                || {
                    if r.purpose == purpose {
                        "no other route matched and capability filter dropped this".to_string()
                    } else {
                        format!("purpose mismatch ({:?} != {:?})", r.purpose, purpose)
                    }
                },
                str::to_string,
            );
            diagnostics.push(DiagnosticEntry {
                route_id: r.id.clone(),
                kept,
                reason,
                origin: None,
                breaker_state: breaker_states[i],
            });
        }
        return Err(RouterError::NoCandidate {
            purpose,
            needs: needs.render(),
        });
    };

    let primary = &routes[primary_idx];

    // Build the ordered candidate list.
    let mut ordered: Vec<Candidate> = Vec::new();
    ordered.push(Candidate {
        route_idx: primary_idx,
        annotation: CandidateAnnotation::for_route(primary, CandidateOrigin::Primary),
    });

    // Track included to avoid duplicates and to mark diagnostics correctly.
    let mut included = vec![false; routes.len()];
    included[primary_idx] = true;

    match primary.fallback.as_ref() {
        Some(ids) => {
            // Explicit fallback list (in order).
            for fid in ids {
                let pos = routes.iter().position(|r| &r.id == fid);
                let Some(p) = pos else {
                    return Err(RouterError::UnknownFallbackId {
                        from: primary.id.clone(),
                        missing: fid.clone(),
                    });
                };
                if included[p] {
                    continue;
                }
                if viability[p].is_some() {
                    continue;
                }
                included[p] = true;
                ordered.push(Candidate {
                    route_idx: p,
                    annotation: CandidateAnnotation::for_route(
                        &routes[p],
                        CandidateOrigin::FallbackId,
                    ),
                });
            }
        }
        None => {
            // Implicit fallback: all other same-purpose viable routes in
            // declaration order.
            for (i, r) in routes.iter().enumerate() {
                if i == primary_idx {
                    continue;
                }
                if r.purpose != purpose {
                    continue;
                }
                if viability[i].is_some() {
                    continue;
                }
                included[i] = true;
                ordered.push(Candidate {
                    route_idx: i,
                    annotation: CandidateAnnotation::for_route(
                        r,
                        CandidateOrigin::ImplicitFallback,
                    ),
                });
            }
        }
    }

    // Build diagnostics for every route.
    for (i, r) in routes.iter().enumerate() {
        if included[i] {
            let origin = ordered
                .iter()
                .find(|c| c.route_idx == i)
                .map(|c| c.annotation.origin);
            diagnostics.push(DiagnosticEntry {
                route_id: r.id.clone(),
                kept: true,
                reason: origin
                    .map_or_else(|| "(unspecified)".to_string(), |o| o.label().to_string()),
                origin,
                breaker_state: breaker_states[i],
            });
        } else {
            let reason = viability[i].map_or_else(
                || {
                    if r.purpose == purpose {
                        "not on this fallback chain".to_string()
                    } else {
                        format!("purpose mismatch ({:?} != {:?})", r.purpose, purpose)
                    }
                },
                str::to_string,
            );
            diagnostics.push(DiagnosticEntry {
                route_id: r.id.clone(),
                kept: false,
                reason,
                origin: None,
                breaker_state: breaker_states[i],
            });
        }
    }

    Ok((ordered, diagnostics))
}
