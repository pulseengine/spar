// SysML v2 Training: Requirement Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/07-*

package RequirementDefinitionTraining {

    requirement def SystemReq {
        doc "Top-level system requirement"
    }

    requirement def PerformanceReq :> SystemReq {
        doc "Performance requirement"
        attribute metric;
        attribute threshold;
    }

    requirement def SafetyReq :> SystemReq {
        doc "Safety requirement"
        attribute severity;
    }

    requirement def InterfaceReq :> SystemReq {
        doc "Interface requirement"
    }

    requirement perfReq1 : PerformanceReq {
        doc "Latency below 10ms"
        attribute metric = "latency";
    }

    requirement safetyReq1 : SafetyReq {
        doc "Fail-safe on power loss"
    }

    part def Controller;
    part def TestHarness;

    satisfy perfReq1 by Controller;
    verify safetyReq1 by TestHarness;
    refine safetyReq1 by perfReq1;
}
