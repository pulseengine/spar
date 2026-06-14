//! PLP — Polynomial-size Linear Program FIFO network-calculus bound
//! (REQ-NC-PLP-001), feed-forward only.
//!
//! # What this computes
//!
//! Where [`crate::tfa`] aggregates every flow at a server and bounds the
//! whole aggregate (cheap, valid under cycles, but loose), and where
//! [`crate::pmoo`] is a closed form restricted to tandems, PLP solves a
//! *single linear program per flow* over a finite set of "interesting"
//! time instants and recovers a delay bound that is provably
//! `EXACT ≤ plp ≤ TFA` and, on feed-forward trees, materially tighter
//! than TFA on the very topologies (lines with overlapping cross-traffic)
//! where PMOO/LUDB do not apply.
//!
//! The formulation is Bouillard's polynomial-size LP (arXiv:2010.09263):
//! a *time-expanded* model whose variables are the cumulative arrival
//! functions `f_i(server, date)` of each flow sampled at a quadratic-size
//! set of dates, plus the dates `t_k` themselves. The objective
//! `max t₀ − t_{source}` is exactly the worst-case horizontal distance
//! (delay) the FIFO + arrival + service constraints permit.
//!
//! # Faithful-port scope and the oracle
//!
//! This module is a direct transcription of panco's *pure* polynomial LP
//! (`FifoLP(polynomial=True, tfa=False)`, `sfa=False`) — Anne Bouillard's
//! own reference implementation — restricted to the case panco handles
//! without forest decomposition: **feed-forward** networks, where every
//! flow's bound is computed on the sink-tree obtained by projecting the
//! network onto the servers that can reach that flow's last hop
//! (`sub_network`). Because the LP is *transcribed* from panco and the
//! exact floor is *also* panco's, the cross-validation in
//! `tests/nc_validation.rs` is **port-fidelity** (a regression pin to
//! solver precision), not an independent soundness proof of the PLP
//! *method* — that is the deferred simulation floor REQ-NC-SIM-FLOOR-001.
//!
//! Two deliberate scope cuts, each its own requirement:
//!
//! * **Generic topology — forks and cycles** (panco's `edges_forest`
//!   decomposition + LP fixpoint, and the "PLP converges where TFA diverges
//!   at high utilization" claim) — REQ-NC-PLP-002. This build projects onto
//!   single-successor sink-trees, so a cycle *or* a fork (the latter still
//!   feed-forward) is rejected here with [`PlpError::NotFeedForward`].
//! * **Tightening to the exact worst case** via panco's TFA-strengthened
//!   LP (`tfa=True`, which embeds per-server TFA delays in the program) —
//!   REQ-NC-PLP-003. The *pure* LP ported here is looser than EXACT on
//!   some trees (e.g. 72.22 vs 68.4 µs on a 3-hop line) but is the only
//!   variant spar can build *byte-identically* to panco from its own
//!   model, which is what makes the solver-precision pin meaningful.
//!
//! # Units
//!
//! Externally this crate is bytes + picoseconds + bits/s (same as
//! [`crate::curves`]). panco's LP is bits + seconds + bits/s. To keep the
//! constraint matrix well-conditioned for HiGHS we run the LP in
//! **bits + microseconds + bits/µs**: a pure rescale of panco's program
//! (`t ↦ t·10⁶`, `ρ ↦ ρ/10⁶`) that leaves every constraint — and hence the
//! optimum, read directly in microseconds — identical. The boundary
//! conversions live in [`plp_bound`]; the optimum is ceiling-rounded to
//! picoseconds so the reported bound never undercuts the LP's continuous
//! optimum.
//!
//! Reference: A. Bouillard, "Trade-off between accuracy and tractability
//! of Network Calculus in FIFO networks", Performance Evaluation, 2021
//! (arXiv:2010.09263); the `panco` reference implementation,
//! `github.com/Huawei-Paris-Research-Center/panco`.

use std::collections::HashMap;

use good_lp::{
    Expression, ProblemVariables, Solution, SolverModel, Variable, constraint, default_solver,
    variable,
};

use crate::curves::{ArrivalCurve, ServiceCurve};

/// Per-link maximum-service ("shaping") rate, in bits/s. Matches the
/// non-binding `TokenBucket(0, 1e12)` the cross-validation oracle pins for
/// every link (`panco_bench.py`'s `BIG`): spar's rate-latency service
/// model carries no real link shaping, so the shaping constraints are
/// emitted with this astronomically large rate (and zero burst) purely to
/// reproduce panco's LP exactly. They are non-binding at the optimum.
const LINK_SHAPING_RATE_BPS: f64 = 1.0e12;

/// A flow through the network: its source arrival curve and the ordered
/// list of servers it crosses (indices into the `services` slice passed to
/// [`plp_bound`]). Identical in shape to [`crate::tfa::TfaFlow`].
#[derive(Debug, Clone)]
pub struct PlpFlow {
    /// Arrival curve of the flow at its source (before any server).
    pub alpha: ArrivalCurve,
    /// Ordered server indices the flow traverses.
    pub path: Vec<usize>,
}

/// Result of a PLP solve over a feed-forward network.
#[derive(Debug, Clone, PartialEq)]
pub struct PlpBound {
    /// Per-flow end-to-end delay bound in picoseconds, indexed like the
    /// input `flows` slice. Each entry is one PLP LP solved on that flow's
    /// sink-tree projection.
    pub flow_delay_ps: Vec<u64>,
}

/// Errors returned by [`plp_bound`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlpError {
    /// No flows, or no servers.
    EmptyNetwork,
    /// A flow path references a server index `>= services.len()`, or is
    /// empty.
    OutOfRange,
    /// The topology is outside this build's scope: either a cycle (truly
    /// not feed-forward), or a server with more than one successor. The
    /// latter is a *fork* — still feed-forward, but this build projects
    /// onto single-successor sink-trees only ([`SubNetwork`]'s successor
    /// relation is single-valued), so branching is rejected here too.
    /// Generic feed-forward (forks) and cyclic topology are REQ-NC-PLP-002.
    NotFeedForward,
    /// Aggregate sustained rate `≥` service rate at some server: the
    /// necessary stability condition fails and no finite bound exists, so
    /// the LP is unbounded. (Mirrors [`crate::tfa::TfaError::Unstable`].)
    Unstable {
        /// Index of the offending server.
        server: usize,
        /// Summed sustained rate of all flows crossing it (bits/s).
        agg_rate_bps: u64,
        /// The server's service rate (bits/s).
        service_rate_bps: u64,
    },
    /// HiGHS returned a non-optimal status (infeasible / unbounded /
    /// numerical). Should not happen on a stable feed-forward network; it
    /// is surfaced rather than silently producing a wrong bound.
    SolverFailed,
}

impl core::fmt::Display for PlpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyNetwork => write!(f, "PLP: no flows or no servers"),
            Self::OutOfRange => {
                write!(f, "PLP: flow path is empty or references an unknown server")
            }
            Self::NotFeedForward => write!(
                f,
                "PLP: topology out of scope (cyclic, or branching/fork beyond single-successor sink-trees) — see REQ-NC-PLP-002"
            ),
            Self::Unstable {
                server,
                agg_rate_bps,
                service_rate_bps,
            } => write!(
                f,
                "PLP: server {server} unstable: aggregate rate {agg_rate_bps} bps >= service rate \
                 {service_rate_bps} bps; bound is unbounded"
            ),
            Self::SolverFailed => write!(f, "PLP: solver returned a non-optimal status"),
        }
    }
}

impl core::error::Error for PlpError {}

/// PLP end-to-end delay bounds for a feed-forward FIFO network.
///
/// `services[h]` is the rate-latency service curve at server `h`;
/// `flows[i].path` lists the servers flow `i` crosses in order. For every
/// flow the network is projected onto the sink-tree of servers that can
/// reach the flow's last hop, and one polynomial LP is solved on that
/// projection. Returns the per-flow delay bounds in picoseconds, or a
/// [`PlpError`].
pub fn plp_bound(flows: &[PlpFlow], services: &[ServiceCurve]) -> Result<PlpBound, PlpError> {
    if flows.is_empty() || services.is_empty() {
        return Err(PlpError::EmptyNetwork);
    }
    let n_servers = services.len();
    for flow in flows {
        if flow.path.is_empty() || flow.path.iter().any(|&h| h >= n_servers) {
            return Err(PlpError::OutOfRange);
        }
    }

    // ── Global topology (panco `topology`) ───────────────────────────────
    // successors[j]: servers reachable in one hop from j (across all flows).
    // predecessors[j]: the reverse. A feed-forward sink-tree has ≤ 1
    // successor per server; > 1 means branching/cyclic → REQ-NC-PLP-002.
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n_servers];
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n_servers];
    for flow in flows {
        for w in flow.path.windows(2) {
            let (a, b) = (w[0], w[1]);
            if !successors[a].contains(&b) {
                successors[a].push(b);
            }
            if !predecessors[b].contains(&a) {
                predecessors[b].push(a);
            }
        }
    }
    for s in &successors {
        if s.len() > 1 {
            return Err(PlpError::NotFeedForward);
        }
    }
    // Cycle detection on the (now functional) successor relation: following
    // the unique successor from any server must terminate.
    if has_cycle(&successors, n_servers) {
        return Err(PlpError::NotFeedForward);
    }

    // Stability precheck (once, on the full network): aggregate sustained
    // rate < service rate at every server. Rates are invariant under PLP's
    // arrival propagation, so a single check suffices and an unstable
    // server would otherwise make the per-flow LP unbounded.
    let mut agg_rate = vec![0u64; n_servers];
    for flow in flows {
        for &h in &flow.path {
            agg_rate[h] = agg_rate[h].saturating_add(flow.alpha.sustained_rate_bps);
        }
    }
    for (h, &rate) in agg_rate.iter().enumerate() {
        if rate != 0 && rate >= services[h].rate_bps {
            return Err(PlpError::Unstable {
                server: h,
                agg_rate_bps: rate,
                service_rate_bps: services[h].rate_bps,
            });
        }
    }

    // ── Per-flow LP on the sink-tree projection ──────────────────────────
    let paths: Vec<&[usize]> = flows.iter().map(|f| f.path.as_slice()).collect();
    let mut flow_delay_ps = Vec::with_capacity(flows.len());
    for foi in 0..flows.len() {
        let sub = SubNetwork::project(foi, flows, services, &paths, &predecessors)?;
        let delay_us = sub.solve()?;
        // Ceiling-round to ps so the reported bound never dips below the
        // LP's continuous optimum (pessimism direction).
        let delay_ps = (delay_us * 1.0e6).ceil();
        let delay_ps = if delay_ps.is_finite() && delay_ps >= 0.0 {
            delay_ps as u64
        } else {
            return Err(PlpError::SolverFailed);
        };
        flow_delay_ps.push(delay_ps);
    }

    Ok(PlpBound { flow_delay_ps })
}

/// Does following the unique successor from any node revisit a node? On a
/// feed-forward network it never does (servers form a rooted forest).
fn has_cycle(successors: &[Vec<usize>], n: usize) -> bool {
    // 0 = unvisited, 1 = on current chain, 2 = settled (acyclic from here).
    let mut state = vec![0u8; n];
    for start in 0..n {
        let mut node = start;
        // Walk the functional successor chain, marking the active path.
        loop {
            match state[node] {
                1 => return true, // back onto the active chain ⇒ cycle
                2 => break,       // joins an already-cleared chain
                _ => state[node] = 1,
            }
            match successors[node].first() {
                Some(&next) => node = next,
                None => break, // reached a sink
            }
        }
        // Clear the chain we just walked (mark settled).
        let mut node = start;
        loop {
            if state[node] != 1 {
                break;
            }
            state[node] = 2;
            match successors[node].first() {
                Some(&next) => node = next,
                None => break,
            }
        }
    }
    false
}

/// The per-flow sink-tree projection plus everything the LP build needs.
/// Server / flow indices below are **local** to this sub-network (panco's
/// `sub_network` re-indexes both, sorted), so the flow of interest always
/// ends at the highest-numbered server (the sink).
struct SubNetwork {
    num_servers: usize,
    /// Local index of the flow of interest (the one whose delay we solve).
    foi: usize,
    /// `path[i]` — local server indices flow `i` crosses, in order.
    path: Vec<Vec<usize>>,
    /// `sigma_bits[i]` — flow `i` burst, in bits.
    sigma_bits: Vec<f64>,
    /// `rho_bpus[i]` — flow `i` sustained rate, in bits per microsecond.
    rho_bpus: Vec<f64>,
    /// `rate_bpus[j]` — server `j` service rate, in bits per microsecond.
    rate_bpus: Vec<f64>,
    /// `latency_us[j]` — server `j` service latency, in microseconds.
    latency_us: Vec<f64>,
    /// `successor[j]` — the unique tree-successor of `j` (`None` at sink).
    successor: Vec<Option<usize>>,
    /// `flows_in_server[j]` — local flows crossing server `j`.
    flows_in_server: Vec<Vec<usize>>,
    /// `edges[(j,h)]` — local flows traversing the directed link `j → h`.
    edges: HashMap<(usize, usize), Vec<usize>>,
    /// `depth[j]` — distance to the sink (sink = 0).
    depth: Vec<usize>,
    /// `t_min[j]`, `t_max[j]` — the date-index window owned by server `j`.
    t_min: Vec<usize>,
    t_max: Vec<usize>,
    /// Number of date variables `t_0 … t_{num_dates-1}`.
    num_dates: usize,
}

impl SubNetwork {
    /// Build the sink-tree projection for flow `foi` (panco `sub_network` +
    /// `topology` + `server_depth` + `times`), in LP units.
    fn project(
        foi: usize,
        flows: &[PlpFlow],
        services: &[ServiceCurve],
        paths: &[&[usize]],
        predecessors: &[Vec<usize>],
    ) -> Result<SubNetwork, PlpError> {
        // backward_search: all servers with a directed path to foi's sink.
        let sink = *paths[foi].last().expect("non-empty path checked");
        let mut keep = vec![false; services.len()];
        let mut stack = vec![sink];
        keep[sink] = true;
        while let Some(s) = stack.pop() {
            for &p in &predecessors[s] {
                if !keep[p] {
                    keep[p] = true;
                    stack.push(p);
                }
            }
        }
        // list_servers / list_flows in panco's sorted order; the global→
        // local server map is `local_of[global]`.
        let list_servers: Vec<usize> = (0..services.len()).filter(|&j| keep[j]).collect();
        let mut local_of = vec![usize::MAX; services.len()];
        for (local, &global) in list_servers.iter().enumerate() {
            local_of[global] = local;
        }
        // Truncated, re-indexed paths; flows with a non-empty projection
        // are kept (foi always is — its whole path reaches the sink).
        let mut path: Vec<Vec<usize>> = Vec::new();
        let mut sigma_bits: Vec<f64> = Vec::new();
        let mut rho_bpus: Vec<f64> = Vec::new();
        let mut local_foi = usize::MAX;
        for (i, flow) in flows.iter().enumerate() {
            let sub: Vec<usize> = paths[i]
                .iter()
                .filter(|&&g| keep[g])
                .map(|&g| local_of[g])
                .collect();
            if sub.is_empty() {
                continue;
            }
            if i == foi {
                local_foi = path.len();
            }
            path.push(sub);
            sigma_bits.push(flow.alpha.burst_bytes as f64 * 8.0);
            rho_bpus.push(flow.alpha.sustained_rate_bps as f64 / 1.0e6);
        }
        debug_assert_ne!(local_foi, usize::MAX, "foi always survives projection");

        let num_servers = list_servers.len();
        let rate_bpus: Vec<f64> = list_servers
            .iter()
            .map(|&g| services[g].rate_bps as f64 / 1.0e6)
            .collect();
        let latency_us: Vec<f64> = list_servers
            .iter()
            .map(|&g| services[g].latency_ps as f64 / 1.0e6)
            .collect();

        // Local topology: successors / predecessors / flows_in_server /
        // edges, all on the projected paths.
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); num_servers];
        let mut flows_in_server: Vec<Vec<usize>> = vec![Vec::new(); num_servers];
        let mut edges: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, p) in path.iter().enumerate() {
            for &j in p {
                flows_in_server[j].push(i);
            }
            for w in p.windows(2) {
                let (a, b) = (w[0], w[1]);
                if !succ[a].contains(&b) {
                    succ[a].push(b);
                }
                edges.entry((a, b)).or_default().push(i);
            }
        }
        // Feed-forward guarantee comes from the global topology check (each
        // server has ≤ 1 successor); the sink is the last server
        // (num_servers-1). `succ` is functional here, so take the head.
        let successor: Vec<Option<usize>> = succ.iter().map(|s| s.first().copied()).collect();

        // server_depth: sink depth 0, each node = successor depth + 1.
        // list_servers is sorted and a node's successor has a higher local
        // index, so a single downward sweep settles all depths.
        let mut depth = vec![0usize; num_servers];
        for j in (0..num_servers.saturating_sub(1)).rev() {
            if let Some(h) = successor[j] {
                depth[j] = depth[h] + 1;
            }
        }

        // times(num_servers, depth) → date windows and total date count.
        let (t_min, t_max, num_dates) = times(num_servers, &depth);

        Ok(SubNetwork {
            num_servers,
            foi: local_foi,
            path,
            sigma_bits,
            rho_bpus,
            rate_bpus,
            latency_us,
            successor,
            flows_in_server,
            edges,
            depth,
            t_min,
            t_max,
            num_dates,
        })
    }

    /// Build and solve the polynomial LP for this sub-network; returns the
    /// foi delay in microseconds (the objective optimum).
    fn solve(&self) -> Result<f64, PlpError> {
        let mut vars = ProblemVariables::new();

        // Date variables t_0 … t_{num_dates-1} (microseconds, ≥ 0).
        let t: Vec<Variable> = (0..self.num_dates)
            .map(|_| vars.add(variable().min(0.0)))
            .collect();

        // Flow cumulative-arrival variables f_{i, server, date}, created
        // lazily as constraints reference them (the referenced server index
        // can be `num_servers`, the virtual output node, and `i` may be a
        // flow that does not traverse that server — both follow panco's LP,
        // where lp_solve auto-declares any named variable as ≥ 0).
        let mut fmap: HashMap<(usize, usize, usize), Variable> = HashMap::new();
        let f = |vars: &mut ProblemVariables,
                 fmap: &mut HashMap<(usize, usize, usize), Variable>,
                 i: usize,
                 s: usize,
                 k: usize|
         -> Variable {
            *fmap
                .entry((i, s, k))
                .or_insert_with(|| vars.add(variable().min(0.0)))
        };

        let mut cons: Vec<good_lp::constraint::Constraint> = Vec::new();

        // /* Time constraints */
        // t1 ≤ t0; t2 ≤ t1; then per non-sink j with successor h, over the
        // depth+1 dates: t_{tj+u+1} ≤ t_{tj+u} and t_{tj+u} ≤ t_{th+u}.
        cons.push(constraint!(t[1] <= t[0]));
        cons.push(constraint!(t[2] <= t[1]));
        for j in 0..self.num_servers {
            let Some(h) = self.successor[j] else { continue };
            let tj = self.t_min[j];
            let th = self.t_min[h];
            for u in 0..=self.depth[j] {
                cons.push(constraint!(t[tj + u + 1] <= t[tj + u]));
                cons.push(constraint!(t[tj + u] <= t[th + u]));
            }
        }

        // /* Arrival constraints */  (x_i replaced by the constant σ_i)
        // f_{i,j,u} − f_{i,j,v} ≤ σ_i + ρ_i·t_u − ρ_i·t_v
        // for j = first server of flow i, t_min[j] ≤ u < v ≤ t_max[j].
        for i in 0..self.path.len() {
            let j = self.path[i][0];
            let rho = self.rho_bpus[i];
            let sigma = self.sigma_bits[i];
            for u in self.t_min[j]..self.t_max[j] {
                for v in (u + 1)..=self.t_max[j] {
                    let fu = f(&mut vars, &mut fmap, i, j, u);
                    let fv = f(&mut vars, &mut fmap, i, j, v);
                    cons.push(constraint!(fu - fv <= sigma + rho * t[u] - rho * t[v]));
                }
            }
        }

        // /* FIFO constraints */
        // last server j (== num_servers-1): f_{i,j,1} = f_{i,j+1,0}
        // else, over u in 0..=depth[j]: f_{i,j,t_min[j]+u} = f_{i,h,t_min[h]+u}.
        for i in 0..self.path.len() {
            for &j in &self.path[i] {
                if j == self.num_servers - 1 {
                    let a = f(&mut vars, &mut fmap, i, j, 1);
                    let b = f(&mut vars, &mut fmap, i, j + 1, 0);
                    cons.push(constraint!(a - b == 0.0));
                } else {
                    let h = self.successor[j].expect("non-sink has a successor");
                    for u in 0..=self.depth[j] {
                        let a = f(&mut vars, &mut fmap, i, j, self.t_min[j] + u);
                        let b = f(&mut vars, &mut fmap, i, h, self.t_min[h] + u);
                        cons.push(constraint!(a - b == 0.0));
                    }
                }
            }
        }

        // /* Service constraints */  (strict service curve, two rows)
        // For server j: u = t_max[j]; sink ⇒ v = 0, h = num_servers; else
        // h = successor, v = t_max[h].
        //   Σ_{i∈fis[j]} (f_{i,h,v} − f_{i,j,u}) + R·T ≥ R·t_v − R·t_u
        //   Σ_{i∈fis[j]} (f_{i,h,v} − f_{i,j,u})       ≥ 0
        for j in 0..self.num_servers {
            let u = self.t_max[j];
            let (h, v) = if j == self.num_servers - 1 {
                (self.num_servers, 0usize)
            } else {
                (
                    self.successor[j].expect("non-sink has a successor"),
                    self.t_max[self.successor[j].unwrap()],
                )
            };
            let r = self.rate_bpus[j];
            let rt = r * self.latency_us[j];
            let mut sum = Expression::from(0.0);
            for &i in &self.flows_in_server[j] {
                let fhv = f(&mut vars, &mut fmap, i, h, v);
                let fju = f(&mut vars, &mut fmap, i, j, u);
                sum += fhv - fju;
            }
            cons.push(constraint!(sum.clone() + rt >= r * t[v] - r * t[u]));
            cons.push(constraint!(sum >= 0.0));
        }

        // /* Monotony constraints */
        // f_{i,j,u} − f_{i,j,u+1} ≥ 0 for j on path i, t_min[j] ≤ u < t_max[j].
        for i in 0..self.path.len() {
            for &j in &self.path[i] {
                for u in self.t_min[j]..self.t_max[j] {
                    let a = f(&mut vars, &mut fmap, i, j, u);
                    let b = f(&mut vars, &mut fmap, i, j, u + 1);
                    cons.push(constraint!(a - b >= 0.0));
                }
            }
        }

        // /* Shaping constraints */ (link max-service, non-binding here)
        // For non-sink j → h, over t_min[h] ≤ u < v ≤ t_max[h]:
        //   Σ_{i∈edges[(j,h)]} (f_{i,h,u} − f_{i,h,v}) ≤ σ_s + ρ_s·t_u − ρ_s·t_v
        // with (σ_s, ρ_s) = (0, LINK_SHAPING_RATE_BPS) in LP units.
        let shaping_rho = LINK_SHAPING_RATE_BPS / 1.0e6;
        for j in 0..self.num_servers {
            let Some(h) = self.successor[j] else { continue };
            let Some(link_flows) = self.edges.get(&(j, h)) else {
                continue;
            };
            for u in self.t_min[h]..self.t_max[h] {
                for v in (u + 1)..=self.t_max[h] {
                    let mut sum = Expression::from(0.0);
                    for &i in link_flows {
                        let fu = f(&mut vars, &mut fmap, i, h, u);
                        let fv = f(&mut vars, &mut fmap, i, h, v);
                        sum += fu - fv;
                    }
                    cons.push(constraint!(sum <= shaping_rho * t[u] - shaping_rho * t[v]));
                }
            }
        }

        // (arrival_shaping, sfa_delay, tfa_delay constraints are all empty
        //  for the pure feed-forward LP: no arrival_shaping groups, and the
        //  sfa/tfa delay vectors are None — see module docs.)

        // /* Objective */  max t_0 − t_{t_min[first server of foi]}.
        let src = self.t_min[self.path[self.foi][0]];
        let objective: Expression = t[0] - t[src];

        let mut model = vars.maximise(objective).using(default_solver);
        for c in cons {
            model = model.with(c);
        }
        let solution = model.solve().map_err(|_| PlpError::SolverFailed)?;
        let delay_us = solution.value(t[0]) - solution.value(t[src]);
        if !delay_us.is_finite() || delay_us < 0.0 {
            return Err(PlpError::SolverFailed);
        }
        Ok(delay_us)
    }
}

/// panco `times(num_servers, depth)`: assign each server a contiguous block
/// of date indices, walking from the sink (highest index) down. Returns
/// `(t_min, t_max, num_dates)` where server `j` owns dates `t_min[j]..=t_max[j]`.
fn times(num_servers: usize, depth: &[usize]) -> (Vec<usize>, Vec<usize>, usize) {
    let mut t_min = vec![0usize; num_servers];
    let mut t_max = vec![0usize; num_servers];
    let mut t = 0usize;
    for j in (0..num_servers).rev() {
        t_min[j] = t + 1;
        t_max[j] = t + (depth[j] + 2);
        t = t_max[j];
    }
    (t_min, t_max, t + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GBPS: u64 = 1_000_000_000;
    const HUNDRED_MBPS: u64 = 100_000_000;
    const US_PS: u64 = 1_000_000; // 1 µs in ps
    const FRAME: u64 = 1500;

    fn flow(burst: u64, rate: u64, path: &[usize]) -> PlpFlow {
        PlpFlow {
            alpha: ArrivalCurve::affine(burst, rate),
            path: path.to_vec(),
        }
    }

    /// Assert each PLP bound is within `tol` (relative) of the pinned panco
    /// pure-PLP value (µs), and `≥` panco's EXACT bound (µs) — the soundness
    /// floor no sound method may undercut. Pinned values are
    /// `FifoLP(polynomial=True, tfa=False)` and `FifoLP(polynomial=False)`
    /// from `tests/oracles/panco/`.
    fn check(
        name: &str,
        flows: &[PlpFlow],
        services: &[ServiceCurve],
        plp_us: &[f64],
        exact_us: &[f64],
        tol: f64,
    ) {
        let r = plp_bound(flows, services).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.flow_delay_ps.len(), plp_us.len(), "{name}: flow count");
        for (i, &got_ps) in r.flow_delay_ps.iter().enumerate() {
            let got_us = got_ps as f64 / US_PS as f64;
            let exact_ps = (exact_us[i] * US_PS as f64).round() as u64;
            // Soundness sandwich floor: EXACT ≤ plp.
            assert!(
                got_ps >= exact_ps,
                "{name} flow {i}: UNSOUND — plp {got_us:.4} µs < exact {:.4} µs",
                exact_us[i]
            );
            // Port-fidelity: match panco's pure PLP to solver precision.
            let rel = (got_us - plp_us[i]).abs() / plp_us[i];
            assert!(
                rel <= tol,
                "{name} flow {i}: plp {got_us:.4} µs disagrees with panco PLP {:.4} µs by {:.3}% (> {:.3}%)",
                plp_us[i],
                rel * 100.0,
                tol * 100.0
            );
        }
    }

    /// fixture 1 — `tandem_cross`: tagged `[0,1]`, cross `[0]`. Pure PLP
    /// equals EXACT here (39.0, 34.0).
    #[test]
    fn tandem_cross() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 5 * US_PS),
        ];
        let flows = vec![
            flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            flow(FRAME, HUNDRED_MBPS, &[0]),
        ];
        check(
            "tandem_cross",
            &flows,
            &services,
            &[39.0, 34.0],
            &[39.0, 34.0],
            1e-4,
        );
    }

    /// fixture 2 — `single_flow_tandem`: one flow `[0,1]`. Pure PLP = EXACT = 27.0.
    #[test]
    fn single_flow_tandem() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 5 * US_PS),
        ];
        let flows = vec![flow(FRAME, HUNDRED_MBPS, &[0, 1])];
        check(
            "single_flow_tandem",
            &flows,
            &services,
            &[27.0],
            &[27.0],
            1e-4,
        );
    }

    /// fixture 3 — `three_server_line`: tagged `[0,1,2]`, cross1 `[0,1]`,
    /// cross2 `[1,2]`. The discriminating fixture: pure PLP `[72.22, 61.11,
    /// 57.98]` is strictly inside the sandwich (EXACT `[68.4, 58.4, 57.98]`,
    /// TFA `[134.7, 86.78, 100.7]`) — looser than EXACT but ≫ tighter than
    /// TFA, on a 3-server *line* where PMOO/LUDB do not apply.
    #[test]
    fn three_server_line() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        let flows = vec![
            flow(FRAME, HUNDRED_MBPS, &[0, 1, 2]),
            flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            flow(FRAME, HUNDRED_MBPS, &[1, 2]),
        ];
        check(
            "three_server_line",
            &flows,
            &services,
            &[72.22, 61.11, 57.98],
            &[68.4, 58.4, 57.98],
            1e-4,
        );
    }

    /// PLP must be ≤ TFA on every fixture flow (the dominance claim). Uses
    /// the 3-server line where the gap is largest.
    #[test]
    fn plp_dominates_tfa_on_line() {
        use crate::tfa::{TfaFlow, tfa_bound};
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        let paths: [&[usize]; 3] = [&[0, 1, 2], &[0, 1], &[1, 2]];
        let plp = plp_bound(
            &paths
                .iter()
                .map(|p| flow(FRAME, HUNDRED_MBPS, p))
                .collect::<Vec<_>>(),
            &services,
        )
        .unwrap();
        let tfa = tfa_bound(
            &paths
                .iter()
                .map(|p| TfaFlow {
                    alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
                    path: p.to_vec(),
                })
                .collect::<Vec<_>>(),
            &services,
        )
        .unwrap();
        for i in 0..3 {
            assert!(
                plp.flow_delay_ps[i] <= tfa.flow_delay_ps[i],
                "flow {i}: plp {} > tfa {}",
                plp.flow_delay_ps[i],
                tfa.flow_delay_ps[i]
            );
        }
    }

    /// A cyclic network (flow A: 0→1, flow B: 1→0) is rejected — PLP on
    /// cyclic topology is REQ-NC-PLP-002, not this module.
    #[test]
    fn cyclic_rejected() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        let flows = vec![
            flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            flow(FRAME, HUNDRED_MBPS, &[1, 0]),
        ];
        assert_eq!(
            plp_bound(&flows, &services).unwrap_err(),
            PlpError::NotFeedForward
        );
    }

    /// Branching (a server with two successors) is also REQ-NC-PLP-002.
    #[test]
    fn branching_rejected() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        // server 0 → {1, 2}: two successors.
        let flows = vec![
            flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            flow(FRAME, HUNDRED_MBPS, &[0, 2]),
        ];
        assert_eq!(
            plp_bound(&flows, &services).unwrap_err(),
            PlpError::NotFeedForward
        );
    }

    #[test]
    fn unstable_rejected() {
        let services = vec![ServiceCurve::rate_latency(HUNDRED_MBPS, 10 * US_PS)];
        let flows = vec![
            flow(FRAME, HUNDRED_MBPS, &[0]),
            flow(FRAME, HUNDRED_MBPS, &[0]),
        ];
        assert!(matches!(
            plp_bound(&flows, &services).unwrap_err(),
            PlpError::Unstable { server: 0, .. }
        ));
    }

    #[test]
    fn empty_and_out_of_range() {
        let services = vec![ServiceCurve::rate_latency(GBPS, 10 * US_PS)];
        assert_eq!(
            plp_bound(&[], &services).unwrap_err(),
            PlpError::EmptyNetwork
        );
        let f = flow(FRAME, HUNDRED_MBPS, &[0]);
        assert_eq!(
            plp_bound(std::slice::from_ref(&f), &[]).unwrap_err(),
            PlpError::EmptyNetwork
        );
        let bad = flow(FRAME, HUNDRED_MBPS, &[0, 5]);
        assert_eq!(
            plp_bound(&[bad], &services).unwrap_err(),
            PlpError::OutOfRange
        );
    }
}
