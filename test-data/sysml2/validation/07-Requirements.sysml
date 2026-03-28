// SysML v2 Validation: Requirements
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package RequirementsValidation {

    requirement def PerformanceReq {
        doc "System performance requirements"
        attribute maxLatency : Real;
    }

    requirement def SafetyReq {
        doc "System safety requirements"
    }

    requirement def ReliabilityReq {
        doc "System reliability requirements"
        attribute mtbf : Real;
    }

    requirement latencyReq : PerformanceReq {
        doc "End-to-end latency must be below 10ms"
        attribute maxLatency = 10;
    }

    requirement safetyReq : SafetyReq {
        doc "System must fail safe"
    }

    requirement reliabilityReq : ReliabilityReq {
        doc "MTBF must exceed 10000 hours"
        attribute mtbf = 10000;
    }

    part def Controller;
    part def SafetyTest;

    satisfy latencyReq by Controller;
    verify safetyReq by SafetyTest;
    refine safetyReq by latencyReq;
}
