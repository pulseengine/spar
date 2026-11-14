-- Spar Scheduling Theory Proofs
--
-- Machine-checked proofs of the mathematical theorems underlying
-- spar's scheduling analysis (crates/spar-analysis/src/scheduling.rs).
--
-- These proofs verify the THEORY, not the code. They guarantee that
-- the mathematical claims our analysis depends on are actually true.
--
-- Verified theorems:
--   1. RTA fixed-point iteration convergence (Joseph & Pandya 1986)
--   2. RM utilization bound soundness (Liu & Layland 1973)
--   3. EDF optimality for implicit-deadline systems (Dertouzos 1974)
--   4. Jittered RTA convergence (Tindell & Clark 1994) — PR #147
--   5. Network Calculus min-plus closed-forms (Le Boudec & Thiran 2001)
--   6. Flow latency monotonicity in component WCETs (Feiertag et al. 2009)
--   7. ARINC 653 partition isolation (ARINC 653P1-5 §3)
--
-- Partly proven: piecewise-affine (T-SPEC) min-plus closed forms,
-- Network/MinPlusPwa.lean. Every theorem there is proven except
-- pwa_backlog_bound_classical, which is still `sorry`. The theorems and
-- defs that still depend on `sorry`, here and in MinPlus.lean, are listed
-- by name in .github/trust-ledger.txt (REQ-PROOF-AXIOM-LEDGER-001).

import Proofs.Scheduling.RTACore
import Proofs.Scheduling.RTA
import Proofs.Scheduling.RMBound
import Proofs.Scheduling.EDF
import Proofs.Scheduling.RTAJitteredCore
import Proofs.Scheduling.RTAJittered
import Proofs.Scheduling.ArincSupply
import Proofs.Scheduling.Latency
import Proofs.Scheduling.Arinc653Isolation
import Proofs.Network.MinPlus
import Proofs.Network.MinPlusPwa
