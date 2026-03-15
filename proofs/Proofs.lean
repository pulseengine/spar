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

import Proofs.Scheduling.RTA
import Proofs.Scheduling.RMBound
import Proofs.Scheduling.EDF
